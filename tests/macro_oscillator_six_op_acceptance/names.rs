//! C12: names.

use std::path::{Path, PathBuf};

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn text_files(relative: &str) -> Vec<(PathBuf, String)> {
    let mut paths = Vec::new();
    files_under(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join(relative),
        &mut paths,
    );
    paths
        .into_iter()
        .filter_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
        .collect()
}

/// Criterion C12 (first part): "no file under `src/modules/macro_oscillator/`
/// or `examples/` contains "dx7", "dx-7" or "yamaha" (case-insensitive)."
/// Assumption: every readable (UTF-8) file under both directories counts,
/// recursively, tests and comments included.
#[test]
fn no_file_names_the_reference_synthesizer() {
    for dir in ["src/modules/macro_oscillator", "examples"] {
        let files = text_files(dir);
        assert!(!files.is_empty(), "no files found under {dir}");
        for (path, text) in files {
            let lower = text.to_lowercase();
            for banned in ["dx7", "dx-7", "yamaha"] {
                assert!(
                    !lower.contains(banned),
                    "{} contains \"{banned}\"",
                    path.display()
                );
            }
        }
    }
}
