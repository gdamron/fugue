//! Starting the decoder that turns a background video into raw RGBA frames.
//!
//! Production runs ffmpeg as a child process. Playback only sees the
//! [`DecoderLauncher`] seam, so tests can feed it frames from memory.

use std::io::Read;
use std::process::{Child, Command, Stdio};

use super::playback::VideoPlaybackConfig;

/// Starts one decode pass over the configured video.
pub(crate) trait DecoderLauncher: Send + Sync {
    fn launch(&self, config: &VideoPlaybackConfig) -> Result<LaunchedDecoder, String>;
}

/// A running decoder: its raw frame output, and the process behind it.
pub(crate) struct LaunchedDecoder {
    /// Back-to-back RGBA frames of the configured size; ends at end of video.
    pub frames: Box<dyn Read + Send>,
    pub process: Box<dyn DecoderProcess>,
}

/// Control over a running decoder from any thread.
pub(crate) trait DecoderProcess: Send {
    /// Stops the decoder; its frame output then ends.
    fn kill(&mut self);

    /// Waits for the decoder to exit: `Err` holds a failed exit status as
    /// text, and `None` means the status could not be read.
    fn wait(&mut self) -> Option<Result<(), String>>;
}

/// Decodes with an ffmpeg child process.
pub(crate) struct FfmpegDecoderLauncher;

impl DecoderLauncher for FfmpegDecoderLauncher {
    fn launch(&self, config: &VideoPlaybackConfig) -> Result<LaunchedDecoder, String> {
        let spec = FfmpegVideoCommandSpec::new(config);
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("could not start background video decoder: {err}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "background video decoder stdout was unavailable".to_string())?;
        Ok(LaunchedDecoder {
            frames: Box::new(stdout),
            process: Box::new(child),
        })
    }
}

impl DecoderProcess for Child {
    fn kill(&mut self) {
        let _ = Child::kill(self);
    }

    fn wait(&mut self) -> Option<Result<(), String>> {
        let status = Child::wait(self).ok()?;
        Some(if status.success() {
            Ok(())
        } else {
            Err(status.to_string())
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct FfmpegVideoCommandSpec {
    pub(super) program: String,
    pub(super) args: Vec<String>,
}

impl FfmpegVideoCommandSpec {
    pub(super) fn new(config: &VideoPlaybackConfig) -> Self {
        let filter = format!(
            "scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2:color=black,fps={}",
            config.width, config.height, config.width, config.height, config.fps
        );
        Self {
            program: config.ffmpeg_path.clone(),
            args: vec![
                "-hide_banner".to_string(),
                "-loglevel".to_string(),
                "error".to_string(),
                "-nostdin".to_string(),
                "-i".to_string(),
                config.path.to_string_lossy().into_owned(),
                "-map".to_string(),
                "0:v:0".to_string(),
                "-an".to_string(),
                "-vf".to_string(),
                filter,
                "-pix_fmt".to_string(),
                "rgba".to_string(),
                "-f".to_string(),
                "rawvideo".to_string(),
                "pipe:1".to_string(),
            ],
        }
    }
}
