//! File checks for a `skill` package: its `SKILL.md` and its examples.
//!
//! Manifest validation ([`crate::pkg::validate()`]) cannot see files, so
//! installers and pack checkers call [`check_skill_package`] once the package
//! is on disk. The entry must be a `SKILL.md` inside the package whose
//! frontmatter gives a host-safe `name` and a `description`; every `examples`
//! path must be an invention or development document in the package. A link
//! in the body to `policies/<package-id>.md` for a policy the manifest does
//! not require is a warning, not a failure.

use std::fmt;
use std::path::{Path, PathBuf};

use super::manifest::{DepRef, EntrySpec, PackageManifest};
use super::package_file::{self, PackageFileError};
use super::skill_frontmatter::{self, Field, Frontmatter};

/// The longest skill `name` agent hosts accept.
pub const MAX_SKILL_NAME_LEN: usize = 64;

/// Why a skill package was refused. Every variant names the path or field it
/// is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillPackageError {
    /// The manifest is not a `skill` package.
    NotSkillPackage { id: String, kind: &'static str },
    /// The entry is outside the package, linked, or missing.
    Entry(PackageFileError),
    /// The entry file is not named `SKILL.md`.
    NotSkillFile(PathBuf),
    /// The entry does not open with a closed YAML frontmatter block.
    NoFrontmatter(PathBuf),
    /// The frontmatter block is not a YAML mapping.
    BadFrontmatter { path: PathBuf, reason: String },
    /// A required frontmatter field is absent, null or empty.
    MissingField { path: PathBuf, field: &'static str },
    /// A required frontmatter field is a number, boolean, list or mapping.
    FieldNotText { path: PathBuf, field: &'static str },
    /// The frontmatter `name` is not lowercase letters, digits and hyphens of
    /// 1 to 64 characters.
    NameNotHostSafe { path: PathBuf, name: String },
    /// An `examples` path is outside the package, linked, or missing.
    Example(PackageFileError),
    /// An example is not an invention or development document.
    InvalidExample { path: PathBuf, reason: String },
    /// A file exists but could not be read as UTF-8 text.
    Unreadable { path: PathBuf, reason: String },
}

impl fmt::Display for SkillPackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillPackageError::NotSkillPackage { id, kind } => {
                write!(f, "{id} is a `{kind}` package, not a `skill` package")
            }
            SkillPackageError::Entry(error) => write!(f, "skill entry {error}"),
            SkillPackageError::NotSkillFile(path) => {
                write!(
                    f,
                    "skill entry `{}` must be a SKILL.md file",
                    path.display()
                )
            }
            SkillPackageError::NoFrontmatter(path) => write!(
                f,
                "skill entry `{}` must open with a YAML frontmatter block (`---` lines) \
                 giving `name` and `description`",
                path.display()
            ),
            SkillPackageError::BadFrontmatter { path, reason } => write!(
                f,
                "skill entry `{}` has unreadable frontmatter: {reason}",
                path.display()
            ),
            SkillPackageError::MissingField { path, field } => write!(
                f,
                "skill entry `{}` frontmatter field `{field}` is missing or empty",
                path.display()
            ),
            SkillPackageError::FieldNotText { path, field } => write!(
                f,
                "skill entry `{}` frontmatter field `{field}` must be text",
                path.display()
            ),
            SkillPackageError::NameNotHostSafe { path, name } => write!(
                f,
                "skill entry `{}` frontmatter field `name` (`{name}`) must be 1 to \
                 {MAX_SKILL_NAME_LEN} lowercase letters, digits and hyphens",
                path.display()
            ),
            SkillPackageError::Example(error) => write!(f, "example {error}"),
            SkillPackageError::InvalidExample { path, reason } => write!(
                f,
                "example `{}` is not an invention or development document: {reason}",
                path.display()
            ),
            SkillPackageError::Unreadable { path, reason } => {
                write!(f, "`{}` cannot be read: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for SkillPackageError {}

/// Something worth fixing in a skill package that does not stop it loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillWarning {
    /// The body links `policies/<package-id>.md` for a policy the manifest's
    /// `policies` does not list, so the linked file will not be installed.
    UndeclaredPolicyLink { link: String, policy: String },
}

impl fmt::Display for SkillWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillWarning::UndeclaredPolicyLink { link, policy } => write!(
                f,
                "SKILL.md links `{link}`, but `policies` does not list `{policy}`"
            ),
        }
    }
}

/// A skill package that passed its file checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCheck {
    /// The `SKILL.md` path.
    pub entry: PathBuf,
    /// The frontmatter `name`.
    pub name: String,
    /// Problems that do not fail the check.
    pub warnings: Vec<SkillWarning>,
}

/// Check a `skill` package's entry and examples under `package_root`.
pub fn check_skill_package(
    package_root: &Path,
    manifest: &PackageManifest,
) -> Result<SkillCheck, SkillPackageError> {
    let entry = skill_entry_path(package_root, manifest)?;
    let text = read_text(&entry)?;
    let split =
        skill_frontmatter::split(&text).ok_or(SkillPackageError::NoFrontmatter(entry.clone()))?;
    let frontmatter = Frontmatter::parse(split.frontmatter).map_err(|reason| {
        SkillPackageError::BadFrontmatter {
            path: entry.clone(),
            reason,
        }
    })?;
    let field = |field: &'static str| match frontmatter.field(field) {
        Field::Text(text) => Ok(text),
        Field::Missing => Err(SkillPackageError::MissingField {
            path: entry.clone(),
            field,
        }),
        Field::NotText => Err(SkillPackageError::FieldNotText {
            path: entry.clone(),
            field,
        }),
    };
    let name = field("name")?;
    field("description")?;
    if !is_host_safe_name(&name) {
        return Err(SkillPackageError::NameNotHostSafe { path: entry, name });
    }
    for example in manifest.examples.iter().flatten() {
        check_example(package_root, example)?;
    }
    let warnings = undeclared_policy_links(split.body, manifest);
    Ok(SkillCheck {
        entry,
        name,
        warnings,
    })
}

/// The `SKILL.md` a skill `manifest` names under `package_root`, located
/// without reading it.
pub(crate) fn skill_entry_path(
    package_root: &Path,
    manifest: &PackageManifest,
) -> Result<PathBuf, SkillPackageError> {
    let EntrySpec::Skill { skill } = &manifest.entry else {
        return Err(SkillPackageError::NotSkillPackage {
            id: manifest.id.clone(),
            kind: manifest.kind.as_str(),
        });
    };
    let relative = Path::new(skill);
    if !package_file::stays_inside(relative) {
        return Err(SkillPackageError::Entry(PackageFileError::OutsidePackage(
            relative.to_path_buf(),
        )));
    }
    if relative.file_name().and_then(|name| name.to_str()) != Some("SKILL.md") {
        return Err(SkillPackageError::NotSkillFile(package_root.join(relative)));
    }
    package_file::locate(package_root, relative).map_err(SkillPackageError::Entry)
}

/// Lowercase ASCII letters, digits and hyphens, 1 to 64 characters: a name
/// every agent host accepts as a skill directory and command name.
pub fn is_host_safe_name(name: &str) -> bool {
    (1..=MAX_SKILL_NAME_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn check_example(package_root: &Path, example: &str) -> Result<(), SkillPackageError> {
    let path = package_file::locate(package_root, Path::new(example))
        .map_err(SkillPackageError::Example)?;
    let text = crate::invention::file::read(&path)
        .map_err(|err| SkillPackageError::Unreadable {
            reason: match err {
                crate::invention::file::DocumentFileError::NotText { .. } => {
                    "not UTF-8 text".to_string()
                }
                err => err.to_string(),
            },
            path: path.clone(),
        })?
        .text;
    crate::invention::Invention::from_json(&text).map_err(|err| {
        SkillPackageError::InvalidExample {
            path,
            reason: err.to_string(),
        }
    })?;
    Ok(())
}

fn read_text(path: &Path) -> Result<String, SkillPackageError> {
    let unreadable = |reason: String| SkillPackageError::Unreadable {
        path: path.to_path_buf(),
        reason,
    };
    let bytes =
        super::read_limit::read_document(path).map_err(|err| unreadable(err.to_string()))?;
    String::from_utf8(bytes).map_err(|_| unreadable("not UTF-8 text".to_string()))
}

/// Links in `body` to `policies/<package-id>.md` whose id the manifest's
/// `policies` does not list, once each, in order of appearance. Fenced code
/// blocks are skipped.
fn undeclared_policy_links(body: &str, manifest: &PackageManifest) -> Vec<SkillWarning> {
    let declared: Vec<String> = manifest
        .policy_requirements()
        .iter()
        .filter_map(|raw| DepRef::parse(raw).map(|dep| dep.id))
        .collect();
    let mut warnings = Vec::new();
    for link in link_targets(body) {
        let Some(policy) = linked_policy(&link) else {
            continue;
        };
        if declared.iter().any(|id| id == policy) {
            continue;
        }
        let warning = SkillWarning::UndeclaredPolicyLink {
            policy: policy.to_string(),
            link: link.clone(),
        };
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    warnings
}

/// The policy id a link target names when it has the form
/// `policies/<package-id>.md` (optionally `./`-prefixed, with a fragment).
fn linked_policy(target: &str) -> Option<&str> {
    let target = target.split(['#', '?']).next().unwrap_or_default();
    let target = target.strip_prefix("./").unwrap_or(target);
    let id = target.strip_prefix("policies/")?.strip_suffix(".md")?;
    super::validate::is_valid_id(id).then_some(id)
}

/// Inline link targets (`[text](target)`) and reference definitions
/// (`[label]: target`) outside fenced code blocks.
fn link_targets(body: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut fence: Option<&str> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some("```");
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some("~~~");
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('[') {
            if let Some((_, definition)) = rest.split_once("]:") {
                if let Some(target) = definition.split_whitespace().next() {
                    targets.push(unwrap_angle(target).to_string());
                }
                continue;
            }
        }
        let mut rest = line;
        while let Some(start) = rest.find("](") {
            rest = &rest[start + 2..];
            let target = rest
                .split(|c: char| c == ')' || c.is_whitespace())
                .next()
                .unwrap_or_default();
            if !target.is_empty() {
                targets.push(unwrap_angle(target).to_string());
            }
        }
    }
    targets
}

fn unwrap_angle(target: &str) -> &str {
    target
        .strip_prefix('<')
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or(target)
}

#[cfg(test)]
#[path = "skill_entry_tests.rs"]
mod tests;
