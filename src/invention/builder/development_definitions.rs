//! Reading the definitions of the developments a document declares.

use std::path::{Path, PathBuf};

use crate::invention::format::Invention;

/// Loads a development's definition: from its file for path-based specs
/// (relative to the parent document's source path) or by cloning an inline
/// definition. Shared with reload, which re-resolves definitions to detect
/// changes on disk.
pub(crate) fn load_development_definition(
    invention: &Invention,
    development: &crate::invention::format::DevelopmentSpec,
) -> Result<Invention, Box<dyn std::error::Error>> {
    let definition = read_development_definition(invention, development)?;
    crate::invention::format::refuse_partial_ranges(
        &format!("Development '{}': ", development.name),
        &definition,
    )?;
    Ok(definition)
}

fn read_development_definition(
    invention: &Invention,
    development: &crate::invention::format::DevelopmentSpec,
) -> Result<Invention, Box<dyn std::error::Error>> {
    if let Some(reference) = &development.reference {
        if development.path.is_some() || development.definition.is_some() {
            return Err(
                "invalid_request: Development requires exactly one of path, definition or ref"
                    .into(),
            );
        }
        #[cfg(not(target_arch = "wasm32"))]
        return Ok(
            crate::pkg::content::ContentRoots::from_environment()?.load_development(reference)?
        );
        #[cfg(target_arch = "wasm32")]
        {
            let _ = reference;
            return Err("Catalog references require a daemon filesystem".into());
        }
    }
    match (&development.path, &development.definition) {
        (Some(path), None) => {
            let resolved = resolve_development_path(invention.source_path.as_deref(), path)?;
            Invention::from_file(&resolved.to_string_lossy())
        }
        (None, Some(definition)) => Ok((**definition).clone()),
        (Some(_), Some(_)) => Err(format!(
            "Development '{}' must set either 'path' or 'definition', not both",
            development.name
        )
        .into()),
        (None, None) => Err(format!(
            "Development '{}' must set either 'path' or 'definition'",
            development.name
        )
        .into()),
    }
}

pub(crate) fn resolve_development_path(
    source_path: Option<&Path>,
    path: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        return Ok(candidate);
    }

    let Some(source_path) = source_path else {
        return Err(format!(
            "Relative development path '{}' requires the parent invention to be loaded from a file",
            path
        )
        .into());
    };

    let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    Ok(base_dir.join(candidate))
}
