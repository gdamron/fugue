//! Pure transitive dependency resolution.
//!
//! Given a package's declared `deps` (`id@requirement`), this walks the
//! transitive graph, picks a concrete version for each id, and records the
//! resolved edges. All I/O — fetching candidate versions and package contents
//! from a registry, git, or local path — lives behind the [`PackageProvider`]
//! trait, so this module stays free of network/filesystem concerns and is
//! shared by every client (the CLI today, the daemon when install moves to RPC).
//!
//! This is deliberately **not** a backtracking solver (see FUG-94 / FUG-100):
//! each id is pinned to the highest version satisfying the first requirement
//! reached for it, and a later incompatible requirement on the same id is
//! reported as a conflict rather than triggering re-selection. Full
//! range-solving is deferred until a multi-version registry index exists.

use std::collections::{BTreeMap, VecDeque};
use std::error::Error;

use semver::{Version, VersionReq};

use crate::pkg::{DepRef, LockSource, PackageManifest};

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

/// Resolve the transitive closure of `root_deps` (excluding the root package),
/// returning resolved packages in discovery order.
pub fn resolve_transitive<P: PackageProvider>(
    root_deps: &[String],
    provider: &P,
) -> Result<Vec<Resolved<P::Acquired>>, Box<dyn Error>> {
    let mut chosen: BTreeMap<String, Version> = BTreeMap::new();
    let mut acquired: BTreeMap<String, (PackageManifest, P::Acquired)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut queue: VecDeque<DepRef> = VecDeque::new();
    for raw in root_deps {
        queue.push_back(parse_dep(raw)?);
    }

    while let Some(dep) = queue.pop_front() {
        let req = VersionReq::parse(&dep.requirement)
            .map_err(|err| format!("invalid version requirement '{}': {err}", dep.requirement))?;
        if let Some(version) = chosen.get(&dep.id) {
            if !req.matches(version) {
                return Err(format!(
                    "dependency conflict: {} is pinned to {version} but a requirement needs {}",
                    dep.id, dep.requirement
                )
                .into());
            }
            continue;
        }
        let available = provider.available_versions(&dep.id)?;
        let version = select_version(&available, &req).ok_or_else(|| {
            format!(
                "no available version of {} satisfies {}",
                dep.id, dep.requirement
            )
        })?;
        let (manifest, contents) = provider.acquire(&dep.id, &version)?;
        for raw in &manifest.deps {
            queue.push_back(parse_dep(raw)?);
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
        let dependencies = dependency_edges(&manifest.deps, &versions);
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

fn parse_dep(raw: &str) -> Result<DepRef, Box<dyn Error>> {
    DepRef::parse(raw).ok_or_else(|| format!("invalid dependency reference '{raw}'").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn ver(v: &str) -> Version {
        Version::parse(v).unwrap()
    }

    #[test]
    fn select_version_picks_highest_satisfying() {
        let available = vec![ver("1.0.0"), ver("1.4.0"), ver("2.0.0")];
        assert_eq!(
            select_version(&available, &VersionReq::parse("^1.0").unwrap()),
            Some(ver("1.4.0"))
        );
        assert_eq!(
            select_version(&available, &VersionReq::parse(">=3.0").unwrap()),
            None
        );
    }

    #[test]
    fn dependency_edges_maps_declared_deps_to_resolved_versions() {
        let versions = BTreeMap::from([
            ("a".to_string(), "1.2.0".to_string()),
            ("b".to_string(), "0.3.0".to_string()),
        ]);
        let edges = dependency_edges(&["a@^1.0".into(), "b@~0.3".into()], &versions);
        assert_eq!(edges, vec!["a@1.2.0".to_string(), "b@0.3.0".to_string()]);
    }

    /// A provider with in-memory fixtures: id -> (version, manifest deps).
    struct FakeProvider {
        packages: HashMap<String, Vec<(Version, Vec<String>)>>,
        acquired: RefCell<Vec<String>>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                packages: HashMap::new(),
                acquired: RefCell::new(Vec::new()),
            }
        }

        fn add(mut self, id: &str, version: &str, deps: &[&str]) -> Self {
            self.packages
                .entry(id.to_string())
                .or_default()
                .push((ver(version), deps.iter().map(|d| d.to_string()).collect()));
            self
        }

        fn deps_of(&self, id: &str, version: &Version) -> Vec<String> {
            self.packages[id]
                .iter()
                .find(|(v, _)| v == version)
                .map(|(_, deps)| deps.clone())
                .unwrap_or_default()
        }
    }

    fn manifest_with_deps(id: &str, version: &str, deps: &[String]) -> PackageManifest {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "version": version,
            "kind": "development",
            "license": "MIT",
            "authors": [{"name": "Test"}],
            "targets": ["in-graph-agent"],
            "deps": deps,
            "entry": {"development": "voice.json"},
        }))
        .unwrap()
    }

    impl PackageProvider for FakeProvider {
        type Acquired = ();

        fn available_versions(&self, id: &str) -> Result<Vec<Version>, Box<dyn Error>> {
            Ok(self
                .packages
                .get(id)
                .map(|entries| entries.iter().map(|(v, _)| v.clone()).collect())
                .unwrap_or_default())
        }

        fn acquire(
            &self,
            id: &str,
            version: &Version,
        ) -> Result<(PackageManifest, ()), Box<dyn Error>> {
            self.acquired.borrow_mut().push(format!("{id}@{version}"));
            let deps = self.deps_of(id, version);
            Ok((manifest_with_deps(id, &version.to_string(), &deps), ()))
        }
    }

    #[test]
    fn resolves_transitive_graph() {
        let provider = FakeProvider::new()
            .add("a", "1.0.0", &["b@^0.3"])
            .add("b", "0.3.1", &[]);
        let resolved = resolve_transitive(&["a@^1.0".into()], &provider).unwrap();
        let ids: Vec<_> = resolved.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(resolved[0].version, "1.0.0");
        assert_eq!(resolved[0].dependencies, vec!["b@0.3.1".to_string()]);
        assert_eq!(resolved[1].version, "0.3.1");
    }

    #[test]
    fn satisfied_repeat_requirement_is_not_reacquired() {
        let provider = FakeProvider::new()
            .add("a", "1.0.0", &["c@^1.0"])
            .add("b", "1.0.0", &["c@>=1.0"])
            .add("c", "1.5.0", &[]);
        let resolved =
            resolve_transitive(&["a@^1.0".into(), "b@^1.0".into()], &provider).unwrap();
        assert_eq!(
            provider
                .acquired
                .borrow()
                .iter()
                .filter(|s| s.starts_with("c@"))
                .count(),
            1
        );
        assert!(resolved.iter().any(|r| r.id == "c" && r.version == "1.5.0"));
    }

    #[test]
    fn incompatible_requirements_conflict() {
        let provider = FakeProvider::new()
            .add("a", "1.0.0", &[])
            .add("a", "2.0.0", &[]);
        let err = match resolve_transitive(&["a@^1.0".into(), "a@^2.0".into()], &provider) {
            Err(err) => err,
            Ok(_) => panic!("expected a dependency conflict"),
        };
        assert!(err.to_string().contains("dependency conflict"));
    }

    #[test]
    fn missing_satisfying_version_errors() {
        let provider = FakeProvider::new().add("a", "1.0.0", &[]);
        let err = match resolve_transitive(&["a@^2.0".into()], &provider) {
            Err(err) => err,
            Ok(_) => panic!("expected a missing-version error"),
        };
        assert!(err.to_string().contains("no available version"));
    }
}
