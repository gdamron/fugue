//! Pure transitive dependency resolution.
//!
//! Given a package's declared `deps` and `policies` (`id@requirement`), this walks the
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

use crate::pkg::{DepRef, LockSource, PackageKind, PackageManifest};

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

/// Resolve `id@version` edges for everything a package requires — its `deps`
/// then its `policies` — from the chosen-version map, without duplicates. This
/// is the `dependencies` list a lockfile entry records.
pub fn locked_edges(
    manifest: &PackageManifest,
    versions: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut edges = dependency_edges(&manifest.deps, versions);
    for edge in dependency_edges(manifest.policy_requirements(), versions) {
        if !edges.contains(&edge) {
            edges.push(edge);
        }
    }
    edges
}

/// Resolve the transitive closure of `root_deps` (excluding the root package),
/// returning resolved packages in discovery order. Policies the resolved
/// packages require are resolved too; use [`resolve_package`] when the root
/// itself may require policies.
pub fn resolve_transitive<P: PackageProvider>(
    root_deps: &[String],
    provider: &P,
) -> Result<Vec<Resolved<P::Acquired>>, Box<dyn Error>> {
    resolve_requirements(root_deps, &[], provider)
}

/// Resolve everything `root` requires, its `deps` and its `policies`, and
/// their transitive requirements (excluding the root package itself).
///
/// A policy is chosen and locked exactly like a dependency, once per id
/// however many packages require it, and must resolve to a `policy` package.
/// Every failure names the id it is about, and comes before anything is
/// staged or loaded.
pub fn resolve_package<P: PackageProvider>(
    root: &PackageManifest,
    provider: &P,
) -> Result<Vec<Resolved<P::Acquired>>, Box<dyn Error>> {
    resolve_requirements(&root.deps, root.policy_requirements(), provider)
}

/// Whether a requirement came from `deps` (any kind) or `policies` (must be a
/// `policy` package).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequiredAs {
    Dependency,
    Policy,
}

struct Requirement {
    dep: DepRef,
    required_as: RequiredAs,
}

fn requirements<'a>(
    deps: &'a [String],
    policies: &'a [String],
) -> impl Iterator<Item = Result<Requirement, Box<dyn Error>>> + 'a {
    let deps = deps.iter().map(|raw| {
        Ok(Requirement {
            dep: parse_dep(raw)?,
            required_as: RequiredAs::Dependency,
        })
    });
    let policies = policies.iter().map(|raw| {
        let dep = DepRef::parse(raw).ok_or_else(|| format!("invalid policy reference '{raw}'"))?;
        Ok(Requirement {
            dep,
            required_as: RequiredAs::Policy,
        })
    });
    deps.chain(policies)
}

fn resolve_requirements<P: PackageProvider>(
    root_deps: &[String],
    root_policies: &[String],
    provider: &P,
) -> Result<Vec<Resolved<P::Acquired>>, Box<dyn Error>> {
    let mut chosen: BTreeMap<String, Version> = BTreeMap::new();
    let mut acquired: BTreeMap<String, (PackageManifest, P::Acquired)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut queue: VecDeque<Requirement> = VecDeque::new();
    for requirement in requirements(root_deps, root_policies) {
        queue.push_back(requirement?);
    }

    while let Some(Requirement { dep, required_as }) = queue.pop_front() {
        let policy = required_as == RequiredAs::Policy;
        let label = if policy { "policy " } else { "" };
        let req = VersionReq::parse(&dep.requirement).map_err(|err| {
            format!(
                "invalid version requirement '{}' for {label}{}: {err}",
                dep.requirement, dep.id
            )
        })?;
        if let Some(version) = chosen.get(&dep.id) {
            if !req.matches(version) {
                let conflict = if policy { "policy" } else { "dependency" };
                return Err(format!(
                    "{conflict} conflict: {} is pinned to {version} but a requirement needs {}",
                    dep.id, dep.requirement
                )
                .into());
            }
            if policy {
                ensure_policy_kind(&dep.id, &acquired[&dep.id].0)?;
            }
            continue;
        }
        let available = provider
            .available_versions(&dep.id)
            .map_err(|err| with_policy_context(policy, &dep.id, err))?;
        if policy && available.is_empty() {
            return Err(format!("no package provides policy {}", dep.id).into());
        }
        let version = select_version(&available, &req).ok_or_else(|| {
            format!(
                "no available version of {label}{} satisfies {}",
                dep.id, dep.requirement
            )
        })?;
        let (manifest, contents) = provider
            .acquire(&dep.id, &version)
            .map_err(|err| with_policy_context(policy, &dep.id, err))?;
        if policy {
            ensure_policy_kind(&dep.id, &manifest)?;
        }
        for requirement in requirements(&manifest.deps, manifest.policy_requirements()) {
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

fn ensure_policy_kind(id: &str, manifest: &PackageManifest) -> Result<(), Box<dyn Error>> {
    if manifest.kind == PackageKind::Policy {
        return Ok(());
    }
    Err(format!(
        "{id} is required as a policy but is a `{}` package",
        manifest.kind.as_str()
    )
    .into())
}

fn with_policy_context(policy: bool, id: &str, err: Box<dyn Error>) -> Box<dyn Error> {
    if policy {
        format!("policy {id}: {err}").into()
    } else {
        err
    }
}

fn parse_dep(raw: &str) -> Result<DepRef, Box<dyn Error>> {
    DepRef::parse(raw).ok_or_else(|| format!("invalid dependency reference '{raw}'").into())
}

#[cfg(test)]
mod tests;
