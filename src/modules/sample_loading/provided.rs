//! Package samples a host hands the engine as bytes, for an engine with no
//! package cache to read them from (the browser engine).
//!
//! A host provides each file of a package it carries, named by the package
//! id, its version and the file's path in it. A document's package ref
//! (`id@requirement:file`) then resolves to the highest provided version of
//! that file whose version meets the requirement, and the sample loads from
//! the provided bytes through the shared decoder and cache. Unlike the
//! package cache, which picks the highest installed version and then needs
//! the file in it, a version counts only for the files provided at it: a
//! host may provide just the files a piece plays.
//!
//! The resolved source is itself a package ref pinned to the version
//! (`id@=version:file`), so resolving it again names the same sample.
//! Everything here runs on a control thread: providing, resolving and
//! reading the bytes all happen while a module builds or a write loads.

// Only the browser engine provides and resolves; native builds read the
// package cache, and reach this only through tests.
#![cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]

use std::collections::BTreeMap;
use std::sync::Arc;

use semver::{Version, VersionReq};

use crate::audio_thread::ControlLock;
use crate::pkg::PackageAudioRef;

/// The largest file one provided sample may be: the cap the package cache
/// reads an audio asset under.
pub(crate) const MAX_PROVIDED_BYTES: usize =
    crate::pkg::read_limit::AUDIO_ASSET_READ_LIMIT as usize;

/// Provided files by package id, then version, then path.
type Provided = BTreeMap<String, BTreeMap<Version, BTreeMap<String, Arc<[u8]>>>>;

static PROVIDED: ControlLock<Provided> = ControlLock::new(BTreeMap::new());

fn provided() -> std::sync::MutexGuard<'static, Provided> {
    PROVIDED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Records `bytes` as the file `path` of package `id` at `version`.
/// Returns the source the file now resolves to (`id@=version:path`).
/// Providing a file again with the same bytes changes nothing; other bytes
/// for a file already provided are refused, since a published version's
/// files never change and its decode may already be cached.
pub(crate) fn provide(
    id: &str,
    version: &str,
    path: &str,
    bytes: Vec<u8>,
) -> Result<String, String> {
    record(id, version, path, bytes).map(|(source, _)| source)
}

/// [`provide`], also saying whether the file is new here (`false` when the
/// same bytes were already provided).
pub(crate) fn record(
    id: &str,
    version: &str,
    path: &str,
    bytes: Vec<u8>,
) -> Result<(String, bool), String> {
    let version = Version::parse(version)
        .map_err(|err| format!("package version '{version}' is not a version: {err}"))?;
    let source = pinned(id, &version, path);
    if PackageAudioRef::parse(&source).is_none() {
        return Err(format!(
            "'{id}' and '{path}' do not name a package file (an id of letters, digits, '.', \
             '-' and '_', and a clean relative path)"
        ));
    }
    if bytes.len() > MAX_PROVIDED_BYTES {
        return Err(format!(
            "sample {source} is {} bytes; a provided sample may be at most {MAX_PROVIDED_BYTES}",
            bytes.len()
        ));
    }
    let mut provided = provided();
    let files = provided
        .entry(id.to_string())
        .or_default()
        .entry(version)
        .or_default();
    let new = match files.get(path) {
        Some(held) if **held == *bytes => false,
        Some(_) => {
            return Err(format!(
                "sample {source} was already provided with other bytes; a package version's \
                 files do not change"
            ))
        }
        None => {
            files.insert(path.to_string(), Arc::from(bytes));
            true
        }
    };
    Ok((source, new))
}

/// Forgets the file `source` names, if it was provided (a provide whose
/// bytes did not decode, say).
pub(crate) fn withdraw(source: &str) {
    let Some((id, version, path)) = parts(source) else {
        return;
    };
    let mut provided = provided();
    if let Some(versions) = provided.get_mut(&id) {
        if let Some(files) = versions.get_mut(&version) {
            files.remove(&path);
            if files.is_empty() {
                versions.remove(&version);
            }
        }
        if versions.is_empty() {
            provided.remove(&id);
        }
    }
}

/// The provided file `reference` resolves to: the highest provided version
/// of its package that meets its requirement and has its file, as a pinned
/// source. `None` when no provided file matches.
pub(crate) fn resolve(reference: &PackageAudioRef) -> Option<String> {
    let requirement = VersionReq::parse(&reference.requirement).ok()?;
    let provided = provided();
    let (version, _) = provided
        .get(&reference.id)?
        .iter()
        .rev()
        .find(|(version, files)| {
            requirement.matches(version) && files.contains_key(&reference.path)
        })?;
    Some(pinned(&reference.id, version, &reference.path))
}

/// The bytes provided for a pinned `source`, if it names a provided file.
pub(crate) fn bytes(source: &str) -> Option<Arc<[u8]>> {
    let (id, version, path) = parts(source)?;
    provided().get(&id)?.get(&version)?.get(&path).cloned()
}

/// Why a package ref did not resolve on an engine whose samples are
/// provided: the files of its package that were, or that none were.
pub(crate) fn unresolved(reference: &PackageAudioRef) -> String {
    let provided = provided();
    let versions = provided
        .get(&reference.id)
        .map(|versions| {
            versions
                .keys()
                .map(Version::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if versions.is_empty() {
        format!(
            "package sample '{reference}' was not provided to this engine: no file of \
             package '{}' was",
            reference.id
        )
    } else {
        format!(
            "package sample '{reference}' was not provided to this engine: package '{}' was \
             provided at {versions}, without that file at a version meeting '{}'",
            reference.id, reference.requirement
        )
    }
}

fn pinned(id: &str, version: &Version, path: &str) -> String {
    format!("{id}@={version}:{path}")
}

/// The id, version and path of a pinned source (`id@=version:path`).
fn parts(source: &str) -> Option<(String, Version, String)> {
    let reference = PackageAudioRef::parse(source)?;
    let version = Version::parse(reference.requirement.strip_prefix('=')?).ok()?;
    Some((reference.id, version, reference.path))
}

#[cfg(test)]
#[path = "provided_tests.rs"]
mod tests;
