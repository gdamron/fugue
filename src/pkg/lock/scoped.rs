//! Frozen-load verification scoped to the packages an invention uses.
//!
//! [`Lockfile::verify`] byte-hashes every locked package, so a frozen load's
//! cost grew with the size of the installed library. A package the invention
//! never reaches cannot change what is heard, so frozen loads hash only the
//! packages the invention uses plus their locked dependencies — still full
//! content hashes, never mtime/size stamps.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};

use super::{LockError, Lockfile};
use crate::pkg::content::ContentRoots;
use crate::Invention;

impl Lockfile {
    /// `ids` plus every package reachable from them through the locked
    /// `dependencies` edges (`id@version`). Ids the lockfile does not lock
    /// are kept but contribute no edges.
    pub fn dependency_closure<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> BTreeSet<String> {
        let mut closure = BTreeSet::new();
        let mut pending: Vec<String> = ids.into_iter().map(str::to_string).collect();
        while let Some(id) = pending.pop() {
            if !closure.insert(id.clone()) {
                continue;
            }
            let Some(package) = self.packages.get(&id) else {
                continue;
            };
            for edge in &package.dependencies {
                let dep = edge.split_once('@').map_or(edge.as_str(), |(dep, _)| dep);
                if !closure.contains(dep) {
                    pending.push(dep.to_string());
                }
            }
        }
        closure
    }

    /// Verify, like [`Lockfile::verify`], only the locked packages in the
    /// dependency closure of `ids`.
    pub fn verify_packages<'a>(
        &self,
        packages_dir: &Path,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), LockError> {
        let closure = self.dependency_closure(ids);
        self.verify_ids(packages_dir, closure.iter().map(String::as_str))
    }

    /// [`Lockfile::verify_first`] for a frozen load of `invention`: hashes
    /// only the packages it uses (see [`ContentRoots::used_packages`]) and
    /// their locked dependencies. When the used set cannot be determined it
    /// verifies every locked package rather than skip one that matters.
    pub fn verify_first_for_invention(
        candidates: &[PathBuf],
        packages_dir: &Path,
        invention: &Invention,
    ) -> Result<(), Box<dyn Error>> {
        let Some(path) = candidates.iter().find(|path| path.is_file()) else {
            return Ok(());
        };
        let lockfile = Self::read(path)?;
        let used = ContentRoots::from_environment().ok().and_then(|roots| {
            ContentRoots {
                packages: packages_dir.to_path_buf(),
                ..roots
            }
            .used_packages(invention)
        });
        match used {
            Some(used) => lockfile.verify_packages(packages_dir, used.iter().map(String::as_str)),
            None => lockfile.verify(packages_dir),
        }
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests;
