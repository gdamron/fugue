//! The ffmpeg command line for one streaming session.

use super::FfmpegStreamConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FfmpegCommandSpec {
    pub(super) program: String,
    pub(super) args: Vec<String>,
}

impl FfmpegCommandSpec {
    pub(super) fn for_ports(config: &FfmpegStreamConfig, audio_port: u16, video_port: u16) -> Self {
        let gop_frames = config.fps.saturating_mul(config.gop_duration).max(1);
        let mut args = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "warning".to_string(),
            "-nostdin".to_string(),
            "-f".to_string(),
            "f32le".to_string(),
            "-ar".to_string(),
            config.sample_rate.to_string(),
            "-ac".to_string(),
            "2".to_string(),
            "-i".to_string(),
            format!("tcp://127.0.0.1:{audio_port}"),
            "-f".to_string(),
            "rawvideo".to_string(),
            "-pix_fmt".to_string(),
            "rgba".to_string(),
            "-s".to_string(),
            format!("{}x{}", config.width, config.height),
            "-framerate".to_string(),
            config.fps.to_string(),
            "-i".to_string(),
            format!("tcp://127.0.0.1:{video_port}"),
            "-c:v".to_string(),
            config.video_encoder.clone(),
            "-b:v".to_string(),
            config.video_bitrate.clone(),
            "-g".to_string(),
            gop_frames.to_string(),
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
        ];

        if config.video_encoder == "libx264" {
            args.extend(["-preset".to_string(), "ultrafast".to_string()]);
            if config.constant_video_bitrate {
                args.extend([
                    "-x264-params".to_string(),
                    "nal-hrd=cbr:force-cfr=1".to_string(),
                ]);
            }
        } else if config.video_encoder == "h264_videotoolbox" {
            args.extend([
                "-constant_bit_rate".to_string(),
                "true".to_string(),
                "-realtime".to_string(),
                "true".to_string(),
            ]);
        }

        if config.constant_video_bitrate {
            args.extend([
                "-minrate".to_string(),
                config.video_bitrate.clone(),
                "-maxrate".to_string(),
                config.video_bitrate.clone(),
                "-bufsize".to_string(),
                config.video_bitrate.clone(),
            ]);
        }

        args.extend([
            "-af".to_string(),
            "aresample=async=1000".to_string(),
            "-c:a".to_string(),
            config.audio_encoder.clone(),
            "-b:a".to_string(),
            config.audio_bitrate.clone(),
        ]);

        if let Some(archive_path) = &config.tee_to_disk {
            args.extend([
                "-flags".to_string(),
                "+global_header".to_string(),
                "-map".to_string(),
                "1:v:0".to_string(),
                "-map".to_string(),
                "0:a:0".to_string(),
                "-f".to_string(),
                "tee".to_string(),
                format!(
                    "[f=flv]{}|[onfail=ignore]{}",
                    escape_tee_name(&config.url),
                    escape_tee_name(archive_path)
                ),
            ]);
        } else {
            args.extend(["-f".to_string(), "flv".to_string(), config.url.clone()]);
        }

        Self {
            program: config.ffmpeg_path.clone(),
            args,
        }
    }
}

fn escape_tee_name(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '\'' | '|' | '[' | ']') || character.is_whitespace() {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::ffmpeg::{DEFAULT_AUDIO_BUFFER_FRAMES, DEFAULT_VIDEO_QUEUE_FRAMES};

    fn minimal_config() -> FfmpegStreamConfig {
        FfmpegStreamConfig {
            ffmpeg_path: "ffmpeg".to_string(),
            url: "rtmp://example.test/live/fugue".to_string(),
            tee_to_disk: None,
            width: 640,
            height: 360,
            fps: 30,
            sample_rate: 48_000,
            video_encoder: "libx264".to_string(),
            audio_encoder: "aac".to_string(),
            video_bitrate: "2500k".to_string(),
            audio_bitrate: "128k".to_string(),
            gop_duration: 2,
            constant_video_bitrate: false,
            audio_buffer_frames: DEFAULT_AUDIO_BUFFER_FRAMES,
            video_queue_frames: DEFAULT_VIDEO_QUEUE_FRAMES,
        }
    }

    #[test]
    fn builds_ffmpeg_command_for_raw_audio_video_to_flv() {
        let mut config = minimal_config();
        config.sample_rate = 44_100;
        config.video_bitrate = "3000k".to_string();
        config.audio_bitrate = "160k".to_string();
        config.gop_duration = 3;
        let spec = FfmpegCommandSpec::for_ports(&config, 12_345, 23_456);

        assert_eq!(spec.program, "ffmpeg");
        let args = spec.args.join(" ");
        assert!(args.contains("-f f32le -ar 44100 -ac 2 -i tcp://127.0.0.1:12345"));
        assert!(args.contains(
            "-f rawvideo -pix_fmt rgba -s 640x360 -framerate 30 -i tcp://127.0.0.1:23456"
        ));
        assert!(args.contains("-c:v libx264 -b:v 3000k -g 90 -pix_fmt yuv420p"));
        assert!(args.contains("-c:a aac -b:a 160k -f flv rtmp://example.test/live/fugue"));
        assert!(!args.contains("-minrate"));
    }

    #[test]
    fn builds_constant_bitrate_arguments_when_requested() {
        let mut config = minimal_config();
        config.video_bitrate = "10000k".to_string();
        config.constant_video_bitrate = true;
        let spec = FfmpegCommandSpec::for_ports(&config, 12_345, 23_456);
        let args = spec.args.join(" ");

        assert!(args.contains("-preset ultrafast"));
        assert!(args.contains("-x264-params nal-hrd=cbr:force-cfr=1"));
        assert!(args.contains("-minrate 10000k -maxrate 10000k -bufsize 10000k"));
    }

    #[test]
    fn enables_realtime_constant_bitrate_for_videotoolbox() {
        let mut config = minimal_config();
        config.video_encoder = "h264_videotoolbox".to_string();
        let spec = FfmpegCommandSpec::for_ports(&config, 12_345, 23_456);
        let args = spec.args.join(" ");

        assert!(args.contains("-c:v h264_videotoolbox"));
        assert!(args.contains("-constant_bit_rate true -realtime true"));
        assert!(!args.contains("-preset"));
    }

    #[test]
    fn builds_tee_output_with_explicit_maps_and_escaped_names() {
        let mut config = minimal_config();
        config.url = "rtmp://example.test/live/fugue|backup".to_string();
        config.tee_to_disk = Some("archive captures/session's [mix].mkv".to_string());
        let spec = FfmpegCommandSpec::for_ports(&config, 12_345, 23_456);

        let output_index = spec.args.iter().position(|arg| arg == "tee").unwrap() + 1;
        assert_eq!(
            spec.args[output_index],
            "[f=flv]rtmp://example.test/live/fugue\\|backup|[onfail=ignore]archive\\ captures/session\\'s\\ \\[mix\\].mkv"
        );
        let args = spec.args.join(" ");
        assert!(args.contains("-flags +global_header"));
        assert!(args.contains("-map 1:v:0 -map 0:a:0 -f tee"));
    }
}
