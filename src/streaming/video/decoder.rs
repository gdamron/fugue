//! The ffmpeg command that decodes a background video into raw RGBA frames.

use super::playback::VideoPlaybackConfig;

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
