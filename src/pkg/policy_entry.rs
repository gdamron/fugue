//! File checks for a `policy` package's entry.
//!
//! A policy package's entry is one Markdown file of shared rules with no
//! triggers. Manifest validation ([`crate::pkg::validate()`]) cannot see the
//! file, so installers and pack checkers call [`check_policy_entry`] once the
//! package is on disk: the entry must exist inside the package, end in `.md`,
//! and must not open with a YAML frontmatter block, which an agent host would
//! discover as a skill.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use super::manifest::{EntrySpec, PackageManifest};

/// Why a policy package's entry file was refused. Every variant names the
/// path it is about.
#[derive(Debug)]
pub enum PolicyEntryError {
    /// The manifest is not a `policy` package.
    NotPolicyPackage { id: String, kind: &'static str },
    /// The entry is absolute or climbs out of the package directory.
    OutsidePackage(PathBuf),
    /// The entry does not exist or is not a regular file.
    Missing(PathBuf),
    /// The entry does not end in `.md`.
    NotMarkdown(PathBuf),
    /// The entry opens with a YAML frontmatter block (`---`).
    HasFrontmatter(PathBuf),
    /// The entry exists but could not be read.
    Unreadable { path: PathBuf, reason: String },
}

impl fmt::Display for PolicyEntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyEntryError::NotPolicyPackage { id, kind } => {
                write!(f, "{id} is a `{kind}` package, not a `policy` package")
            }
            PolicyEntryError::OutsidePackage(path) => write!(
                f,
                "policy entry `{}` must be a relative path inside the package",
                path.display()
            ),
            PolicyEntryError::Missing(path) => {
                write!(
                    f,
                    "policy entry `{}` is missing or not a file",
                    path.display()
                )
            }
            PolicyEntryError::NotMarkdown(path) => write!(
                f,
                "policy entry `{}` must be a Markdown file ending in `.md`",
                path.display()
            ),
            PolicyEntryError::HasFrontmatter(path) => write!(
                f,
                "policy entry `{}` opens with a YAML frontmatter block; a policy has no \
                 triggers, and a host would discover it as a skill",
                path.display()
            ),
            PolicyEntryError::Unreadable { path, reason } => {
                write!(
                    f,
                    "policy entry `{}` cannot be read: {reason}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for PolicyEntryError {}

/// Check a `policy` package's entry file under `package_root`, returning the
/// entry's path on success.
pub fn check_policy_entry(
    package_root: &Path,
    manifest: &PackageManifest,
) -> Result<PathBuf, PolicyEntryError> {
    let EntrySpec::Policy { policy } = &manifest.entry else {
        return Err(PolicyEntryError::NotPolicyPackage {
            id: manifest.id.clone(),
            kind: manifest.kind.as_str(),
        });
    };
    let relative = Path::new(policy);
    let path = package_root.join(relative);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(PolicyEntryError::OutsidePackage(relative.to_path_buf()));
    }
    if relative.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return Err(PolicyEntryError::NotMarkdown(path));
    }
    if !path.is_file() {
        return Err(PolicyEntryError::Missing(path));
    }
    let bytes =
        super::read_limit::read_document(&path).map_err(|err| PolicyEntryError::Unreadable {
            path: path.clone(),
            reason: err.to_string(),
        })?;
    if opens_with_frontmatter(&bytes) {
        return Err(PolicyEntryError::HasFrontmatter(path));
    }
    Ok(path)
}

/// Whether the text's first line (after an optional byte-order mark) is the
/// `---` fence that opens a YAML frontmatter block.
fn opens_with_frontmatter(bytes: &[u8]) -> bool {
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let first_line = text.split(|&b| b == b'\n').next().unwrap_or_default();
    first_line.trim_ascii_end() == b"---"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_manifest(entry: &str) -> PackageManifest {
        serde_json::from_value(serde_json::json!({
            "id": "fugue.test.rules",
            "version": "0.1.0",
            "kind": "policy",
            "license": "CC0-1.0",
            "authors": [{"name": "Test"}],
            "targets": ["external-agent"],
            "entry": {"policy": entry},
        }))
        .unwrap()
    }

    fn package_with(file: &str, text: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(file), text).unwrap();
        dir
    }

    #[test]
    fn accepts_plain_markdown() {
        let dir = package_with("POLICY.md", "# Rules\n\nProtect headroom.\n");
        let path = check_policy_entry(dir.path(), &policy_manifest("POLICY.md")).unwrap();
        assert_eq!(path, dir.path().join("POLICY.md"));
    }

    #[test]
    fn a_later_rule_line_is_not_frontmatter() {
        let dir = package_with("POLICY.md", "# Rules\n\n---\n\nMore.\n");
        check_policy_entry(dir.path(), &policy_manifest("POLICY.md")).unwrap();
    }

    #[test]
    fn refuses_a_missing_entry_naming_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let err = check_policy_entry(dir.path(), &policy_manifest("POLICY.md")).unwrap_err();
        assert!(matches!(err, PolicyEntryError::Missing(_)));
        assert!(err.to_string().contains("POLICY.md"), "{err}");
    }

    #[test]
    fn refuses_a_non_markdown_entry_naming_the_path() {
        let dir = package_with("policy.txt", "rules\n");
        let err = check_policy_entry(dir.path(), &policy_manifest("policy.txt")).unwrap_err();
        assert!(matches!(err, PolicyEntryError::NotMarkdown(_)));
        assert!(err.to_string().contains("policy.txt"), "{err}");
    }

    #[test]
    fn refuses_frontmatter_naming_the_path() {
        for text in [
            "---\nname: x\n---\n# Rules\n",
            "---\r\nname: x\r\n---\r\n",
            "\u{feff}---\nname: x\n---\n",
        ] {
            let dir = package_with("POLICY.md", text);
            let err = check_policy_entry(dir.path(), &policy_manifest("POLICY.md")).unwrap_err();
            assert!(
                matches!(err, PolicyEntryError::HasFrontmatter(_)),
                "{text:?}"
            );
            assert!(err.to_string().contains("POLICY.md"), "{err}");
        }
    }

    #[test]
    fn refuses_an_entry_outside_the_package() {
        let dir = tempfile::tempdir().unwrap();
        let err = check_policy_entry(dir.path(), &policy_manifest("../POLICY.md")).unwrap_err();
        assert!(matches!(err, PolicyEntryError::OutsidePackage(_)));
        assert!(err.to_string().contains("../POLICY.md"), "{err}");
    }

    #[test]
    fn refuses_a_package_of_another_kind() {
        let mut manifest = policy_manifest("POLICY.md");
        manifest.entry = EntrySpec::Skill {
            skill: "SKILL.md".into(),
        };
        manifest.kind = crate::pkg::PackageKind::Skill;
        let dir = tempfile::tempdir().unwrap();
        let err = check_policy_entry(dir.path(), &manifest).unwrap_err();
        assert!(matches!(err, PolicyEntryError::NotPolicyPackage { .. }));
    }
}
