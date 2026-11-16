//! Locating a file a manifest names inside its package.
//!
//! Entry files and other manifest paths are read from an unpacked package, so
//! each one must be a regular file reached through real directories under the
//! package root: the lock's content hash covers only those, and installs never
//! copy links. The policy and skill entry checks share these rules.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Why a path a manifest names was refused. Every variant names the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageFileError {
    /// The path is empty or absolute, climbs out of the package directory, or
    /// lies under `.git`, which the lock's content hash skips.
    OutsidePackage(PathBuf),
    /// The file, or a directory on its path, is a symbolic link.
    Linked(PathBuf),
    /// The file does not exist or is not a regular file.
    Missing(PathBuf),
}

impl fmt::Display for PackageFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageFileError::OutsidePackage(path) => write!(
                f,
                "`{}` must be a relative path inside the package, outside `.git`",
                path.display()
            ),
            PackageFileError::Linked(path) => write!(
                f,
                "`{}` is or passes through a symbolic link; it must be a regular file in \
                 the package",
                path.display()
            ),
            PackageFileError::Missing(path) => {
                write!(f, "`{}` is missing or not a file", path.display())
            }
        }
    }
}

impl std::error::Error for PackageFileError {}

/// Whether `relative` is a non-empty relative path that stays inside the
/// package and outside `.git`.
pub(crate) fn stays_inside(relative: &Path) -> bool {
    !relative.as_os_str().is_empty()
        && relative.components().all(|component| match component {
            // Any case: on a case-insensitive filesystem `.GIT` is `.git`.
            Component::Normal(name) => !name.eq_ignore_ascii_case(".git"),
            Component::CurDir => true,
            _ => false,
        })
}

/// The regular file `relative` names under `package_root`, walked without
/// following links.
pub(crate) fn locate(package_root: &Path, relative: &Path) -> Result<PathBuf, PackageFileError> {
    if !stays_inside(relative) {
        return Err(PackageFileError::OutsidePackage(relative.to_path_buf()));
    }
    let path = package_root.join(relative);
    let mut current = package_root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(PackageFileError::Linked(path));
            }
            Ok(_) => {}
            Err(_) => return Err(PackageFileError::Missing(path)),
        }
    }
    if !path.is_file() {
        return Err(PackageFileError::Missing(path));
    }
    Ok(path)
}

/// The text after an optional UTF-8 byte-order mark.
pub(crate) fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// Whether the text's first line (after an optional byte-order mark) is the
/// `---` fence that opens a YAML frontmatter block.
pub(crate) fn opens_with_frontmatter(bytes: &[u8]) -> bool {
    let first_line = strip_bom(bytes)
        .split(|&b| b == b'\n')
        .next()
        .unwrap_or_default();
    first_line.trim_ascii_end() == b"---"
}
