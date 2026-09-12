//! Session snapshots, deterministic paging and bounded development detail.

use super::*;

/// Refreshes from disk before every read, detecting external installs without watchers.
pub struct ContentCatalog {
    roots: ContentRoots,
    session: String,
    generation: String,
    sequence: u64,
    fingerprint: String,
    entries: Vec<ContentEntry>,
    diagnostics: Vec<ContentError>,
    diagnostics_truncated: bool,
}
impl ContentCatalog {
    /// Create an empty session-local catalog. No indexing runs on the audio thread.
    pub fn new(roots: ContentRoots, session: impl Into<String>) -> Self {
        Self {
            roots,
            session: session.into(),
            generation: String::new(),
            sequence: 0,
            fingerprint: String::new(),
            entries: Vec::new(),
            diagnostics: Vec::new(),
            diagnostics_truncated: false,
        }
    }

    fn refresh(&mut self) -> Result<()> {
        let _guard = package_read_guard(&self.roots.packages)?;
        let mut refs = Vec::new();
        for id in directory(&self.roots.packages)? {
            if !id.file_type().map_err(io_error)?.is_dir() {
                continue;
            }
            for version in directory(&id.path())? {
                if !version.file_type().map_err(io_error)?.is_dir() {
                    continue;
                }
                let version_name = version.file_name().to_string_lossy().into_owned();
                if semver::Version::parse(&version_name).is_err() {
                    continue;
                }
                let path = version.path().join("fugue.pkg.json");
                if let Ok(m) = contained(&version.path(), &path)
                    .and_then(|path| pkg::parse_path(&path).map_err(|e| err("invalid_content", e)))
                {
                    if !matches!(
                        m.kind,
                        pkg::PackageKind::Development | pkg::PackageKind::Invention
                    ) {
                        continue;
                    }
                }
                refs.push(ContentRef::Package {
                    package: id.file_name().to_string_lossy().into_owned(),
                    version: version_name,
                });
            }
        }
        workspace_refs(&self.roots.workspace, &self.roots.workspace, &mut refs, 0)?;
        if refs.len() > MAX_FILES {
            return Err(err(
                "catalog_unavailable",
                "Catalog exceeds 10000 candidates",
            ));
        }
        let mut entries = Vec::new();
        let mut diagnostics = Vec::new();
        let mut diagnostics_truncated = false;
        let mut fingerprints = BTreeMap::new();
        for reference in refs {
            match self.roots.inspect(&reference) {
                Ok(resolved) => {
                    fingerprints.extend(resolved.fingerprints);
                    entries.push(resolved.entry);
                }
                Err(mut error) => {
                    if diagnostics.len() < 20 {
                        error.message = truncate(&error.message, 512);
                        diagnostics.push(error);
                    } else {
                        diagnostics_truncated = true;
                    }
                }
            }
        }
        entries.sort_by(|a, b| {
            a.id.cmp(&b.id).then_with(|| {
                match (
                    semver::Version::parse(&a.version),
                    semver::Version::parse(&b.version),
                ) {
                    (Ok(a), Ok(b)) => a.cmp(&b),
                    _ => a.version.cmp(&b.version),
                }
            })
        });
        let fingerprint = hash(
            &serde_json::to_vec(&(&entries, &diagnostics, diagnostics_truncated, fingerprints))
                .map_err(|e| err("catalog_unavailable", e))?,
        );
        if fingerprint != self.fingerprint {
            self.sequence += 1;
            self.generation = format!("{}:{}", self.session, self.sequence);
            self.fingerprint = fingerprint;
        }
        self.entries = entries;
        self.diagnostics = diagnostics;
        self.diagnostics_truncated = diagnostics_truncated;
        Ok(())
    }

    /// Return a bounded deterministic page. Cursors expire after any catalog change.
    pub fn list(&mut self, query: &ContentListQuery) -> Result<ContentPage> {
        if query.schema_version != 1 || !(1..=100).contains(&query.limit) {
            return Err(err(
                "invalid_request",
                "Use schema_version 1 and limit 1–100",
            ));
        }
        self.refresh()?;
        let filter = hash(&serde_json::to_vec(&(query.kind, query.source, &query.id)).unwrap());
        let prefix = format!("{}:{filter}:", self.generation);
        let offset = match &query.cursor {
            None => 0,
            Some(cursor) => cursor
                .strip_prefix(&prefix)
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or_else(|| err("stale_cursor", "Restart list without a cursor"))?,
        };
        let entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| {
                query.kind.is_none_or(|k| entry.kind == k)
                    && query.source.is_none_or(|s| entry.source == s)
                    && query.id.as_ref().is_none_or(|id| entry.id == *id)
            })
            .collect();
        if offset > entries.len() {
            return Err(err("stale_cursor", "Restart list without a cursor"));
        }
        let mut page = ContentPage {
            schema_version: 1,
            generation: self.generation.clone(),
            items: Vec::new(),
            next_cursor: None,
            diagnostics: self.diagnostics.clone(),
            diagnostics_truncated: self.diagnostics_truncated,
        };
        for (index, entry) in entries.iter().enumerate().skip(offset).take(query.limit) {
            page.items.push((*entry).clone());
            page.next_cursor =
                (index + 1 < entries.len()).then(|| format!("{prefix}{}", index + 1));
            if serde_json::to_vec(&page).unwrap().len() > LIST_BYTES {
                page.items.pop();
                if page.items.is_empty() {
                    return Err(err(
                        "response_too_large",
                        "One catalog entry exceeds the list byte limit",
                    ));
                }
                page.next_cursor = Some(format!("{prefix}{index}"));
                break;
            }
        }
        Ok(page)
    }

    /// Revalidate and inspect a reference without building or playing a graph.
    pub fn detail(&mut self, query: &ContentDetailQuery) -> Result<ContentDetail> {
        if query.schema_version != 1 {
            return Err(err("invalid_request", "Use schema_version 1"));
        }
        validate_reference(&query.reference)?;
        self.refresh()?;
        let _guard = package_read_guard(&self.roots.packages)?;
        let resolved = self.roots.inspect(&query.reference)?;
        let interface = resolved
            .document
            .is_development()
            .then(|| ContentInterface {
                inputs: resolved.document.inputs,
                outputs: resolved.document.outputs,
                controls: resolved.document.controls,
            });
        let detail = ContentDetail {
            schema_version: 1,
            generation: self.generation.clone(),
            entry: resolved.entry,
            interface,
            dependencies: resolved.dependencies,
            assets: resolved.assets,
        };
        if serde_json::to_vec(&detail).unwrap().len() > DETAIL_BYTES {
            return Err(err("response_too_large", "Detail exceeds 256 KiB"));
        }
        Ok(detail)
    }
}
