//! Retrieving `audio_file_sink` output from an offline render (wasm hosts).

use super::RenderEngine;

impl RenderEngine {
    pub fn finish_audio_file_sink(
        &self,
        module_id: &str,
    ) -> Result<crate::AudioFileSinkStats, Box<dyn std::error::Error>> {
        let handle = self.audio_file_sink_handle(module_id)?;
        Ok(handle.finish())
    }

    pub fn audio_file_sink_wav_bytes(
        &self,
        module_id: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let handle = self.audio_file_sink_handle(module_id)?;
        handle.wav_bytes().map_err(Into::into)
    }

    fn audio_file_sink_handle(
        &self,
        module_id: &str,
    ) -> Result<crate::AudioFileSinkHandle, Box<dyn std::error::Error>> {
        let key = format!("{}.handle", module_id);
        self.handles
            .lock()
            .unwrap()
            .get::<crate::AudioFileSinkHandle>(&key)
            .ok_or_else(|| format!("unknown audio_file_sink handle: {}", module_id).into())
    }
}
