//! Resolving an agent package into the prompt, model and tools it runs with.
//!
//! [`resolve_agent`] takes an agent reference (`id@requirement`) and a
//! packages directory; [`resolve_agent_package`] takes an agent package on
//! disk instead. Either reads only installed packages (see
//! [`InstalledPackages`]), under the lockfile governing the calling thread
//! when a frozen session has one, and never the network. It runs on a
//! control thread: on the audio thread it refuses (and panics in debug
//! builds), and it shares no lock with audio code. It holds the packages
//! directory's shared read lock while it reads, so an install never commits
//! halfway through a resolution.
//!
//! The resolved system prompt is, in order and separated by blank lines:
//!
//! 1. the definition's persona (`system_prompt`);
//! 2. each policy once, in first-required order: the agent's own `policies`,
//!    then each of its skills' `policies` in `skills` order;
//! 3. each skill's `SKILL.md` body, without its frontmatter, in `skills`
//!    order.
//!
//! Every failure comes before anything is loaded and names the package it is
//! about.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use semver::VersionReq;

use super::agent_entry::{definition_path, parse_definition};
use super::installed::{read_governed, read_manifest, InstalledPackages};
use super::manifest::{DepRef, PackageKind, PackageManifest};
use super::resolve::{resolve_package, select_version, PackageProvider, Resolved};

/// What a part of the resolved prompt came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSource {
    /// The agent definition's `system_prompt`.
    Persona,
    /// A policy package's entry.
    Policy,
    /// A skill package's `SKILL.md` body.
    Skill,
}

/// One part of a resolved prompt, and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPart {
    /// What the part is.
    pub source: PromptSource,
    /// The package it came from.
    pub package: String,
    /// That package's resolved version.
    pub version: String,
    /// Its length in characters, as joined into the prompt.
    pub chars: usize,
}

/// An agent resolved for a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAgent {
    /// The agent package's id.
    pub id: String,
    /// Its resolved version.
    pub version: String,
    /// The composed system prompt.
    pub system_prompt: String,
    /// The prompt's parts, in prompt order.
    pub parts: Vec<PromptPart>,
    /// The definition's `default_model`; `None` leaves the host's choice.
    pub model: Option<String>,
    /// The definition's `default_tools`, in order.
    pub tools: Vec<String>,
}

/// Why an agent could not be resolved: the agent asked for, and a reason
/// naming the package at fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResolveError {
    /// The agent reference or package resolved.
    pub agent: String,
    /// What went wrong, naming the package it is about.
    pub reason: String,
}

impl fmt::Display for AgentResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot resolve agent {}: {}", self.agent, self.reason)
    }
}

impl std::error::Error for AgentResolveError {}

/// Resolve the installed agent `reference` (`id@requirement`, a semver
/// requirement such as `^0.1.0` or `=0.1.2`) from `packages_dir`.
pub fn resolve_agent(
    reference: &str,
    packages_dir: &Path,
) -> Result<ResolvedAgent, AgentResolveError> {
    let refuse = |reason: String| AgentResolveError {
        agent: reference.to_string(),
        reason,
    };
    let _reading = begin(packages_dir).map_err(refuse)?;
    under_one_lock(|| {
        let provider = InstalledPackages::new(packages_dir);
        let (manifest, dir) = installed_agent(reference, &provider)?;
        compose(&manifest, &dir, &provider)
    })
    .map_err(refuse)
}

/// Resolve the agent package at `package_root`, its requirements from
/// `packages_dir`.
pub fn resolve_agent_package(
    package_root: &Path,
    packages_dir: &Path,
) -> Result<ResolvedAgent, AgentResolveError> {
    let refuse = |agent: String| {
        move |reason: String| AgentResolveError {
            agent: agent.clone(),
            reason,
        }
    };
    let _reading = begin(packages_dir).map_err(refuse(package_root.display().to_string()))?;
    under_one_lock(|| {
        let manifest =
            read_manifest(package_root).map_err(refuse(package_root.display().to_string()))?;
        let provider = InstalledPackages::new(packages_dir);
        compose(&manifest, package_root, &provider)
            .map_err(refuse(format!("{}@{}", manifest.id, manifest.version)))
    })
}

/// Run a whole resolution under the lock governing this thread as it starts,
/// so a session that adopts another lock meanwhile cannot have versions
/// chosen under one lock and bytes verified under another.
fn under_one_lock<T>(body: impl FnOnce() -> T) -> T {
    super::lock::govern_here(super::lock::governing(), body)
}

/// Refuse the audio thread, then take the packages dir's shared read lock.
fn begin(packages_dir: &Path) -> Result<Option<std::fs::File>, String> {
    crate::audio_thread::debug_assert_control_thread("resolving an agent");
    if crate::audio_thread::on_audio_thread() {
        return Err("agents are resolved on a control thread, not the audio thread".into());
    }
    super::content::package_read_guard(packages_dir)
        .map_err(|err| format!("cannot read {}: {err}", packages_dir.display()))
}

/// The installed agent package `reference` names: its manifest and dir.
fn installed_agent(
    reference: &str,
    provider: &InstalledPackages<'_>,
) -> Result<(PackageManifest, PathBuf), String> {
    let dep = DepRef::parse(reference)
        .ok_or_else(|| format!("`{reference}` is not an `id@requirement` reference"))?;
    let requirement = VersionReq::parse(&dep.requirement).map_err(|err| {
        format!(
            "invalid version requirement '{}' for {}: {err}",
            dep.requirement, dep.id
        )
    })?;
    let available = provider
        .available_versions(&dep.id)
        .map_err(|err| format!("{}: {err}", dep.id))?;
    if available.is_empty() {
        return Err(format!("{} is not installed", dep.id));
    }
    let version = select_version(&available, &requirement).ok_or_else(|| {
        format!(
            "no installed version of {} satisfies {}",
            dep.id, dep.requirement
        )
    })?;
    provider
        .acquire(&dep.id, &version)
        .map_err(|err| format!("{}: {err}", dep.id))
}

/// Resolve `manifest`, an agent package in `dir`, against `provider`.
fn compose(
    manifest: &PackageManifest,
    dir: &Path,
    provider: &InstalledPackages<'_>,
) -> Result<ResolvedAgent, String> {
    if manifest.kind != PackageKind::Agent {
        return Err(format!(
            "{} is a `{}` package, not an `agent` package",
            manifest.id,
            manifest.kind.as_str()
        ));
    }
    let named = |err: &dyn fmt::Display| format!("{}: {err}", manifest.id);
    let path = definition_path(dir, manifest).map_err(|err| named(&err))?;
    let bytes = read_governed(&path).map_err(|err| named(&err))?;
    let definition = parse_definition(&path, &bytes, manifest).map_err(|err| named(&err))?;
    let resolved: BTreeMap<String, Resolved<PathBuf>> = resolve_package(manifest, provider)
        .map_err(|err| with_lockfile(err.to_string(), provider))?
        .into_iter()
        .map(|package| (package.id.clone(), package))
        .collect();

    let mut prompt = Prompt::default();
    prompt.push(
        PromptSource::Persona,
        &manifest.id,
        &manifest.version,
        &definition.system_prompt,
    );
    let skills = ids(manifest.skill_requirements());
    let mut policies = ids(manifest.policy_requirements());
    for skill in &skills {
        for policy in ids(resolved[skill].manifest.policy_requirements()) {
            if !policies.contains(&policy) {
                policies.push(policy);
            }
        }
    }
    for policy in &policies {
        let package = &resolved[policy];
        prompt.push(
            PromptSource::Policy,
            policy,
            &package.version,
            &policy_text(package)?,
        );
    }
    for skill in &skills {
        let package = &resolved[skill];
        prompt.push(
            PromptSource::Skill,
            skill,
            &package.version,
            &skill_body(package)?,
        );
    }
    Ok(ResolvedAgent {
        id: manifest.id.clone(),
        version: manifest.version.clone(),
        system_prompt: prompt.text,
        parts: prompt.parts,
        model: definition.default_model,
        tools: definition.default_tools,
    })
}

/// The ids of `requirements`, each once, in order. They parsed when the
/// manifest validated.
fn ids(requirements: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for dep in requirements.iter().filter_map(|raw| DepRef::parse(raw)) {
        if !ids.contains(&dep.id) {
            ids.push(dep.id);
        }
    }
    ids
}

/// A policy package's entry text.
fn policy_text(package: &Resolved<PathBuf>) -> Result<String, String> {
    let named = |err: &dyn fmt::Display| format!("policy {}: {err}", package.id);
    let path = super::policy_entry::policy_entry_path(&package.acquired, &package.manifest)
        .map_err(|err| named(&err))?;
    let bytes = read_governed(&path).map_err(|err| named(&err))?;
    super::policy_entry::check_policy_text(&path, &bytes).map_err(|err| named(&err))?;
    utf8(&path, bytes).map_err(|err| named(&err))
}

/// A skill package's `SKILL.md` body, without its frontmatter.
fn skill_body(package: &Resolved<PathBuf>) -> Result<String, String> {
    let named = |err: &dyn fmt::Display| format!("skill {}: {err}", package.id);
    let path = super::skill_entry::skill_entry_path(&package.acquired, &package.manifest)
        .map_err(|err| named(&err))?;
    let bytes = read_governed(&path).map_err(|err| named(&err))?;
    let text = utf8(&path, bytes).map_err(|err| named(&err))?;
    let split = super::skill_frontmatter::split(&text).ok_or_else(|| {
        named(&format!(
            "{} has no frontmatter block to strip",
            path.display()
        ))
    })?;
    Ok(split.body.to_string())
}

fn utf8(path: &Path, bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text", path.display()))
}

/// Name the governing lockfile in a resolution failure, when one governs.
fn with_lockfile(reason: String, provider: &InstalledPackages<'_>) -> String {
    match provider.lockfile() {
        Some(lockfile) if !reason.contains(&lockfile.display().to_string()) => {
            format!("{reason} (lockfile {})", lockfile.display())
        }
        _ => reason,
    }
}

/// A prompt under construction.
#[derive(Default)]
struct Prompt {
    text: String,
    parts: Vec<PromptPart>,
}

impl Prompt {
    /// Append `text`, trimmed, as a part from `package@version`. An empty
    /// part is reported but adds nothing to the prompt.
    fn push(&mut self, source: PromptSource, package: &str, version: &str, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            if !self.text.is_empty() {
                self.text.push_str("\n\n");
            }
            self.text.push_str(text);
        }
        self.parts.push(PromptPart {
            source,
            package: package.to_string(),
            version: version.to_string(),
            chars: text.chars().count(),
        });
    }
}

#[cfg(test)]
#[path = "agent_resolve_tests.rs"]
mod tests;
