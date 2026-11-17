//! Pure transitive dependency resolution.
//!
//! Given a package's declared `deps`, `policies` and `skills` (`id@requirement`), this walks the
//! transitive graph, picks a concrete version for each id, and records the
//! resolved edges. All I/O — fetching candidate versions and package contents
//! from a registry, git, or local path — lives behind the [`PackageProvider`]
//! trait, so this module stays free of network/filesystem concerns and is
//! shared by every client (the CLI today, the daemon when install moves to RPC).
//!
//! This is deliberately **not** a backtracking solver:
//! each id is pinned to the highest version satisfying the first requirement
//! reached for it, and a later incompatible requirement on the same id is
//! reported as a conflict rather than triggering re-selection. Full
//! range-solving is deferred until a multi-version registry index exists.

use std::collections::{BTreeMap, VecDeque};
use std::error::Error;

use semver::{Version, VersionReq};

use crate::pkg::{DepRef, LockSource, PackageKind, PackageManifest, Target};

/// Supplies candidate versions and package contents during resolution.
///
/// Implementors own all acquisition I/O; the associated [`Acquired`] type is an
/// opaque handle to the fetched contents (e.g. a directory the caller will
/// stage), threaded back out through [`Resolved::acquired`].
///
/// [`Acquired`]: PackageProvider::Acquired
pub trait PackageProvider {
    /// Opaque handle to acquired package contents, returned to the caller.
    type Acquired;

    /// All versions of `id` the source advertises (unsorted, possibly empty).
    fn available_versions(&self, id: &str) -> Result<Vec<Version>, Box<dyn Error>>;

    /// Acquire a specific version, returning its manifest and a contents handle.
    fn acquire(
        &self,
        id: &str,
        version: &Version,
    ) -> Result<(PackageManifest, Self::Acquired), Box<dyn Error>>;
}

/// One fully-resolved transitive dependency, carrying the provider's acquired
/// contents handle so the caller can stage it.
pub struct Resolved<A> {
    /// Package id.
    pub id: String,
    /// Chosen concrete version.
    pub version: String,
    /// Where the package was resolved from.
    pub source: LockSource,
    /// The package's parsed manifest.
    pub manifest: PackageManifest,
    /// The provider's handle to the acquired contents.
    pub acquired: A,
    /// Resolved `id@version` edges of this package.
    pub dependencies: Vec<String>,
}

/// Pick the highest available version satisfying `req`.
pub fn select_version(available: &[Version], req: &VersionReq) -> Option<Version> {
    available.iter().filter(|v| req.matches(v)).max().cloned()
}

/// Resolve `id@version` dependency edges for a package's declared deps from the
/// chosen-version map. Unparseable deps and ids absent from the map are skipped
/// (the latter cannot occur for a fully-resolved graph).
pub fn dependency_edges(deps: &[String], versions: &BTreeMap<String, String>) -> Vec<String> {
    deps.iter()
        .filter_map(|raw| {
            let dep = DepRef::parse(raw)?;
            let version = versions.get(&dep.id)?;
            Some(format!("{}@{}", dep.id, version))
        })
        .collect()
}

/// Resolve `id@version` edges for everything a package requires — its `deps`,
/// then its `policies`, then its `skills` — from the chosen-version map,
/// without duplicates. This is the `dependencies` list a lockfile entry
/// records.
pub fn locked_edges(
    manifest: &PackageManifest,
    versions: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut edges = dependency_edges(&manifest.deps, versions);
    let required = [
        manifest.policy_requirements(),
        manifest.skill_requirements(),
    ];
    for edge in required
        .iter()
        .flat_map(|requirements| dependency_edges(requirements, versions))
    {
        if !edges.contains(&edge) {
            edges.push(edge);
        }
    }
    edges
}

/// Resolve the transitive closure of `root_deps` (excluding the root package),
/// returning resolved packages in discovery order. Policies and skills the
/// resolved packages require are resolved too; use [`resolve_package`] when
/// the root itself may require policies or skills.
pub fn resolve_transitive<P: PackageProvider>(
    root_deps: &[String],
    provider: &P,
) -> Result<Vec<Resolved<P::Acquired>>, Box<dyn Error>> {
    resolve_requirements(Requirements::of_deps(root_deps), provider)
}

/// Resolve everything `root` requires, its `deps`, `policies` and `skills`,
/// and their transitive requirements (excluding the root package itself).
///
/// A policy or skill is chosen and locked exactly like a dependency, once per
/// id however many packages require it. A policy must resolve to a `policy`
/// package, and a skill to a `skill` package that targets `in-graph-agent`.
/// Every failure names the id it is about, and comes before anything is
/// staged or loaded.
pub fn resolve_package<P: PackageProvider>(
    root: &PackageManifest,
    provider: &P,
) -> Result<Vec<Resolved<P::Acquired>>, Box<dyn Error>> {
    resolve_requirements(Requirements::of(root), provider)
}

/// Which manifest field a requirement came from: `deps` (any kind),
/// `policies` (must be a `policy` package) or `skills` (must be a `skill`
/// package for in-graph agents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequiredAs {
    Dependency,
    Policy,
    Skill,
}

impl RequiredAs {
    /// How a requirement of this field is named in messages: `""`,
    /// `"policy "` or `"skill "`.
    fn label(self) -> &'static str {
        match self {
            RequiredAs::Dependency => "",
            RequiredAs::Policy => "policy ",
            RequiredAs::Skill => "skill ",
        }
    }

    /// Refuse a package that cannot meet a requirement of this field.
    fn ensure_fits(self, id: &str, manifest: &PackageManifest) -> Result<(), Box<dyn Error>> {
        let (wanted, field) = match self {
            RequiredAs::Dependency => return Ok(()),
            RequiredAs::Policy => (PackageKind::Policy, "policy"),
            RequiredAs::Skill => (PackageKind::Skill, "skill"),
        };
        if manifest.kind != wanted {
            return Err(format!(
                "{id} is required as a {field} but is a `{}` package",
                manifest.kind.as_str()
            )
            .into());
        }
        if self == RequiredAs::Skill && !manifest.targets.contains(&Target::InGraphAgent) {
            return Err(format!(
                "{id} is required as a skill but its `targets` lack `in-graph-agent`"
            )
            .into());
        }
        Ok(())
    }

    /// Name the policy or skill a provider error came from. The provider's
    /// error stays reachable as the [`Error::source`], so a caller can still
    /// tell a fetch failure from a damaged package by its type.
    fn with_context(self, id: &str, err: Box<dyn Error>) -> Box<dyn Error> {
        match self {
            RequiredAs::Dependency => err,
            _ => Box::new(RequirementError {
                context: format!("{}{id}", self.label()),
                source: err,
            }),
        }
    }
}

/// A provider error for a required policy or skill, prefixed with what was
/// required. Displays as `<label><id>: <provider error>`.
#[derive(Debug)]
struct RequirementError {
    context: String,
    source: Box<dyn Error>,
}

impl std::fmt::Display for RequirementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.context, self.source)
    }
}

impl Error for RequirementError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

struct Requirement {
    dep: DepRef,
    required_as: RequiredAs,
}

/// A package's requirement fields, read in the order they resolve.
struct Requirements<'a> {
    deps: &'a [String],
    policies: &'a [String],
    skills: &'a [String],
}

impl<'a> Requirements<'a> {
    fn of(manifest: &'a PackageManifest) -> Self {
        Self {
            deps: &manifest.deps,
            policies: manifest.policy_requirements(),
            skills: manifest.skill_requirements(),
        }
    }

    fn of_deps(deps: &'a [String]) -> Self {
        Self {
            deps,
            policies: &[],
            skills: &[],
        }
    }

    fn parsed(self) -> impl Iterator<Item = Result<Requirement, Box<dyn Error>>> + 'a {
        let fields = [
            (self.deps, RequiredAs::Dependency, "dependency"),
            (self.policies, RequiredAs::Policy, "policy"),
            (self.skills, RequiredAs::Skill, "skill"),
        ];
        fields
            .into_iter()
            .flat_map(|(raws, required_as, noun)| {
                raws.iter().map(move |raw| (raw, required_as, noun))
            })
            .map(|(raw, required_as, noun)| {
                let dep = DepRef::parse(raw)
                    .ok_or_else(|| format!("invalid {noun} reference '{raw}'"))?;
                Ok(Requirement { dep, required_as })
            })
    }
}

fn resolve_requirements<P: PackageProvider>(
    root: Requirements<'_>,
    provider: &P,
) -> Result<Vec<Resolved<P::Acquired>>, Box<dyn Error>> {
    let mut chosen: BTreeMap<String, Version> = BTreeMap::new();
    let mut acquired: BTreeMap<String, (PackageManifest, P::Acquired)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut queue: VecDeque<Requirement> = VecDeque::new();
    for requirement in root.parsed() {
        queue.push_back(requirement?);
    }

    while let Some(Requirement { dep, required_as }) = queue.pop_front() {
        let label = required_as.label();
        let req = VersionReq::parse(&dep.requirement).map_err(|err| {
            format!(
                "invalid version requirement '{}' for {label}{}: {err}",
                dep.requirement, dep.id
            )
        })?;
        if let Some(version) = chosen.get(&dep.id) {
            if !req.matches(version) {
                let conflict = match required_as {
                    RequiredAs::Dependency => "dependency",
                    RequiredAs::Policy => "policy",
                    RequiredAs::Skill => "skill",
                };
                return Err(format!(
                    "{conflict} conflict: {} is pinned to {version} but a requirement needs {}",
                    dep.id, dep.requirement
                )
                .into());
            }
            required_as.ensure_fits(&dep.id, &acquired[&dep.id].0)?;
            continue;
        }
        let available = provider
            .available_versions(&dep.id)
            .map_err(|err| required_as.with_context(&dep.id, err))?;
        if required_as != RequiredAs::Dependency && available.is_empty() {
            return Err(format!("no package provides {label}{}", dep.id).into());
        }
        let version = select_version(&available, &req).ok_or_else(|| {
            format!(
                "no available version of {label}{} satisfies {}",
                dep.id, dep.requirement
            )
        })?;
        let (manifest, contents) = provider
            .acquire(&dep.id, &version)
            .map_err(|err| required_as.with_context(&dep.id, err))?;
        required_as.ensure_fits(&dep.id, &manifest)?;
        for requirement in Requirements::of(&manifest).parsed() {
            queue.push_back(requirement?);
        }
        chosen.insert(dep.id.clone(), version);
        order.push(dep.id.clone());
        acquired.insert(dep.id.clone(), (manifest, contents));
    }

    let versions: BTreeMap<String, String> = chosen
        .iter()
        .map(|(id, version)| (id.clone(), version.to_string()))
        .collect();

    let mut out = Vec::with_capacity(order.len());
    for id in order {
        let (manifest, contents) = acquired.remove(&id).expect("resolved id present");
        let version = chosen[&id].to_string();
        let dependencies = locked_edges(&manifest, &versions);
        out.push(Resolved {
            source: LockSource::Registry {
                id: id.clone(),
                version: version.clone(),
            },
            version,
            id,
            manifest,
            acquired: contents,
            dependencies,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
