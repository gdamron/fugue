//! Score importers: external notation formats → `fugue.score.v1`.
//!
//! This is the deterministic reference path for score ingest, shared by every
//! surface that hosts it (CLI, MCP, future in-graph agents). MusicXML is the
//! lossless format notation editors export, so its conversion is the ground
//! truth the agent PDF-import is measured against.
//!
//! Gated behind the `score-import` cargo feature so the audio runtime and
//! wasm builds don't carry the XML/zip dependencies unless a host asks for
//! them.

mod musicxml;

pub use musicxml::{convert_musicxml, ConvertReport, BASE_NOTE};

use super::Score;

/// Converts MusicXML bytes — raw `.musicxml`/`.xml` text or a compressed
/// `.mxl` archive (detected by the zip magic, not the extension) — into a
/// [`Score`] plus a conversion report.
pub fn convert_musicxml_bytes(bytes: &[u8]) -> Result<(Score, ConvertReport), String> {
    let xml = read_musicxml(bytes)?;
    convert_musicxml(&xml)
}

fn read_musicxml(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(b"PK\x03\x04") {
        return read_mxl(bytes);
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "input is not valid UTF-8 MusicXML".to_string())
}

fn read_mxl(bytes: &[u8]) -> Result<String, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|err| format!("input is not a readable .mxl archive: {}", err))?;

    // META-INF/container.xml names the root score file.
    let root_path = match read_entry(&mut archive, "META-INF/container.xml") {
        Ok(container) => container_rootfile(&container),
        Err(_) => None,
    };
    let root_path = match root_path {
        Some(path) => path,
        // Fall back to the first XML entry outside META-INF.
        None => archive
            .file_names()
            .filter(|name| !name.starts_with("META-INF/"))
            .filter(|name| name.ends_with(".xml") || name.ends_with(".musicxml"))
            .min()
            .map(str::to_string)
            .ok_or("the .mxl archive contains no MusicXML entry")?,
    };
    read_entry(&mut archive, &root_path)
        .map_err(|err| format!("cannot read '{}' in the .mxl archive: {}", root_path, err))
}

fn read_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<String, String> {
    let mut entry = archive.by_name(name).map_err(|err| err.to_string())?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut entry, &mut text).map_err(|err| err.to_string())?;
    Ok(text)
}

fn container_rootfile(container: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(container).ok()?;
    doc.descendants()
        .find(|n| n.has_tag_name("rootfile"))
        .and_then(|n| n.attribute("full-path"))
        .map(str::to_string)
}
