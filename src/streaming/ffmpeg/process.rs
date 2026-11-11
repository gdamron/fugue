//! The production encoder session: an ffmpeg process that reads raw audio and
//! video from two local TCP inputs, with its stderr kept for diagnostics.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::command::FfmpegCommandSpec;
use super::worker::{EncoderSession, SessionOutcome, SessionStarter};
use super::{redact_stream_url, FfmpegStreamBackend};

/// Starts each session as an ffmpeg process.
pub(super) struct ProcessStarter;

impl SessionStarter for ProcessStarter {
    fn start(&self, shared: &Arc<FfmpegStreamBackend>) -> Result<Box<dyn EncoderSession>, String> {
        Ok(Box::new(start_session(shared.clone())?))
    }
}

struct FfmpegSession {
    shared: Arc<FfmpegStreamBackend>,
    child: Child,
    audio: TcpStream,
    video: TcpStream,
    stderr_reader: Option<JoinHandle<()>>,
}

impl EncoderSession for FfmpegSession {
    fn check_exit(&mut self) -> Option<SessionOutcome> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(SessionOutcome::UnexpectedExit(exit_message(
                status,
                self.shared.stderr_tail_string(),
            ))),
            Ok(None) => None,
            Err(err) => Some(SessionOutcome::IoError(format!(
                "ffmpeg status check failed: {err}"
            ))),
        }
    }

    fn write_audio(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.audio.write_all(bytes)
    }

    fn write_video(&mut self, frame: &[u8]) -> io::Result<()> {
        self.video.write_all(frame)
    }

    fn close(self: Box<Self>) {
        finish_session(*self);
    }
}

fn start_session(shared: Arc<FfmpegStreamBackend>) -> Result<FfmpegSession, String> {
    let audio_listener =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|err| format!("audio bind failed: {err}"))?;
    let video_listener =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|err| format!("video bind failed: {err}"))?;
    audio_listener
        .set_nonblocking(true)
        .map_err(|err| format!("audio listener nonblocking failed: {err}"))?;
    video_listener
        .set_nonblocking(true)
        .map_err(|err| format!("video listener nonblocking failed: {err}"))?;

    let audio_port = audio_listener
        .local_addr()
        .map_err(|err| err.to_string())?
        .port();
    let video_port = video_listener
        .local_addr()
        .map_err(|err| err.to_string())?
        .port();
    let spec = FfmpegCommandSpec::for_ports(&shared.config, audio_port, video_port);

    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to spawn ffmpeg '{}': {err}", spec.program))?;

    let stderr_reader = child.stderr.take().map(|stderr| {
        let reader_shared = shared.clone();
        thread::spawn(move || {
            let mut stderr = BufReader::new(stderr);
            let mut line = Vec::with_capacity(1024);
            loop {
                line.clear();
                match stderr.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        reader_shared.append_stderr(&line);
                        let line = String::from_utf8_lossy(&line);
                        let line = redact_stream_url(line.into_owned(), &reader_shared.config.url);
                        eprint!("ffmpeg stderr: {line}");
                    }
                    Err(_) => break,
                }
            }
        })
    });

    let (audio, video) = accept_input_streams(&mut child, &audio_listener, &video_listener)?;
    audio
        .set_nonblocking(false)
        .map_err(|err| format!("audio stream blocking mode failed: {err}"))?;
    video
        .set_nonblocking(false)
        .map_err(|err| format!("video stream blocking mode failed: {err}"))?;
    let write_timeout = Some(Duration::from_secs(2));
    audio
        .set_write_timeout(write_timeout)
        .map_err(|err| format!("audio stream write timeout failed: {err}"))?;
    video
        .set_write_timeout(write_timeout)
        .map_err(|err| format!("video stream write timeout failed: {err}"))?;
    audio
        .set_nodelay(true)
        .map_err(|err| format!("audio stream nodelay failed: {err}"))?;
    video
        .set_nodelay(true)
        .map_err(|err| format!("video stream nodelay failed: {err}"))?;

    Ok(FfmpegSession {
        shared,
        child,
        audio,
        video,
        stderr_reader,
    })
}

fn accept_input_streams(
    child: &mut Child,
    audio_listener: &TcpListener,
    video_listener: &TcpListener,
) -> Result<(TcpStream, TcpStream), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut audio = None;
    let mut video = None;

    while audio.is_none() || video.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err("ffmpeg did not connect raw audio/video inputs in time".to_string());
        }

        if audio.is_none() {
            match audio_listener.accept() {
                Ok((stream, _)) => audio = Some(stream),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(format!("audio accept failed: {err}")),
            }
        }
        if video.is_none() {
            match video_listener.accept() {
                Ok((stream, _)) => video = Some(stream),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(format!("video accept failed: {err}")),
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!("ffmpeg exited before connecting inputs: {status}"));
            }
            Ok(None) => {}
            Err(err) => return Err(format!("ffmpeg status check failed: {err}")),
        }

        thread::sleep(Duration::from_millis(5));
    }

    Ok((audio.unwrap(), video.unwrap()))
}

fn finish_session(mut session: FfmpegSession) {
    drop(session.audio);
    drop(session.video);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match session.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = session.child.kill();
                let _ = session.child.wait();
                break;
            }
            Err(_) => break,
        }
    }
    if let Some(reader) = session.stderr_reader.take() {
        let _ = reader.join();
    }
}

/// The error for an encoder that exited on its own, with the tail of what it
/// wrote to stderr.
fn exit_message(status: impl std::fmt::Display, stderr: Option<String>) -> String {
    let mut message = format!("ffmpeg exited unexpectedly: {status}");
    if let Some(stderr) = stderr {
        message.push_str("; stderr: ");
        message.push_str(stderr.trim());
    }
    message
}

#[cfg(test)]
mod tests {
    use super::exit_message;

    #[test]
    fn exit_message_carries_the_trimmed_stderr_tail() {
        assert_eq!(
            exit_message("exit status: 1", Some("  Connection refused\n".to_string())),
            "ffmpeg exited unexpectedly: exit status: 1; stderr: Connection refused"
        );
    }

    #[test]
    fn exit_message_without_stderr_names_only_the_status() {
        assert_eq!(
            exit_message("exit status: 1", None),
            "ffmpeg exited unexpectedly: exit status: 1"
        );
    }
}
