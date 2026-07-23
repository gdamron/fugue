//! Daemon identity and the connect-time handshake used to detect a stale daemon.
//!
//! "One daemon, many clients" means a client (the CLI or the MCP adapter) may
//! attach to a daemon it did not spawn. The per-request [`RPC_SCHEMA_VERSION`]
//! check catches gross wire-schema drift, but not the subtler footgun: a
//! rebuilt daemon that kept the same schema version yet changed behavior (new
//! config keys, fixed bugs). A [`BuildFingerprint`] closes that gap, and
//! [`verify_daemon_identity`] lets a connecting client refuse a mismatch instead
//! of silently driving a stale runtime.
//!
//! [`RPC_SCHEMA_VERSION`]: super::RPC_SCHEMA_VERSION

use std::fmt;

use serde::{Deserialize, Serialize};

/// A build fingerprint for the running `fugue` core.
///
/// `crate_version` is always present; the git fields are populated by `build.rs`
/// only inside a checkout, so a packaged build reports version alone. Two builds
/// are considered the same runtime when every populated field matches.
///
/// Note there is intentionally no build timestamp: the CLI and MCP adapter are
/// compiled separately, and two binaries built from identical source must
/// fingerprint equal or they could never share a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct BuildFingerprint {
    /// The `fugue` core crate version both peers were compiled against — the
    /// coarse signal for behavior compatibility.
    pub crate_version: String,
    /// Short git commit the build came from, when built inside a git checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    /// Whether the working tree carried uncommitted changes at build time. The
    /// signal that distinguishes a dev build from a clean release off the same
    /// commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
}

impl BuildFingerprint {
    /// The fingerprint of the currently running build, captured by `build.rs`.
    pub fn current() -> Self {
        Self {
            crate_version: env!("CARGO_PKG_VERSION").to_string(),
            git_sha: option_env!("FUGUE_GIT_SHA").map(str::to_string),
            dirty: option_env!("FUGUE_GIT_DIRTY").map(|flag| flag == "1"),
        }
    }

    /// A compact human-readable form, e.g. `2026.6.0 (a1b2c3d4e5f6, dirty)`.
    pub fn describe(&self) -> String {
        let mut out = self.crate_version.clone();
        if let Some(sha) = &self.git_sha {
            let dirty = if self.dirty == Some(true) {
                ", dirty"
            } else {
                ""
            };
            out.push_str(&format!(" ({sha}{dirty})"));
        } else if self.dirty == Some(true) {
            out.push_str(" (dirty)");
        }
        out
    }
}

impl fmt::Display for BuildFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// The identity a daemon reports over the [`Hello`] handshake so a client can
/// confirm it reached the daemon it expects.
///
/// [`Hello`]: super::RpcRequestPayload::Hello
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct DaemonIdentity {
    /// The wire schema the daemon speaks.
    pub schema_version: u32,
    /// The build the daemon is running.
    pub build: BuildFingerprint,
    /// Opaque id minted when the daemon process started. A new id means the
    /// daemon restarted — useful for a client distinguishing a fresh session
    /// from the one it was talking to.
    pub session_id: String,
    /// The daemon process id, surfaced in diagnostics and shutdown guidance.
    pub pid: u32,
}

/// Why a daemon's reported identity is incompatible with the local build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityMismatch {
    /// The daemon speaks a different wire schema.
    Schema { local: u32, remote: u32 },
    /// The daemon runs a different build (the schema already matched).
    Build {
        local: BuildFingerprint,
        remote: BuildFingerprint,
    },
}

impl fmt::Display for IdentityMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityMismatch::Schema { local, remote } => write!(
                f,
                "daemon speaks RPC schema {remote}, but this client speaks {local}"
            ),
            IdentityMismatch::Build { local, remote } => write!(
                f,
                "daemon is running build {remote}, but this client is build {local}"
            ),
        }
    }
}

/// Verifies a daemon's reported identity against the local build.
///
/// A connecting client calls this right after the handshake and refuses to
/// drive a mismatched daemon, rather than silently attaching to a stale one.
/// Schema mismatch is reported ahead of build mismatch, since a schema gap is
/// the more fundamental incompatibility.
// The `Build` mismatch carries two fingerprints for a clear message; this is a
// cold connect-time path, so the larger `Err` is not worth boxing the public
// type over.
#[allow(clippy::result_large_err)]
pub fn verify_daemon_identity(
    local_schema: u32,
    local_build: &BuildFingerprint,
    remote: &DaemonIdentity,
) -> Result<(), IdentityMismatch> {
    if remote.schema_version != local_schema {
        return Err(IdentityMismatch::Schema {
            local: local_schema,
            remote: remote.schema_version,
        });
    }
    if &remote.build != local_build {
        return Err(IdentityMismatch::Build {
            local: local_build.clone(),
            remote: remote.build.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::RPC_SCHEMA_VERSION;

    fn build(version: &str, sha: Option<&str>, dirty: Option<bool>) -> BuildFingerprint {
        BuildFingerprint {
            crate_version: version.to_string(),
            git_sha: sha.map(str::to_string),
            dirty,
        }
    }

    fn identity(schema: u32, build: BuildFingerprint) -> DaemonIdentity {
        DaemonIdentity {
            schema_version: schema,
            build,
            session_id: "session".to_string(),
            pid: 1,
        }
    }

    #[test]
    fn matching_identity_verifies() {
        let local = build("2026.6.0", Some("abc123"), Some(false));
        let remote = identity(RPC_SCHEMA_VERSION, local.clone());
        assert_eq!(
            verify_daemon_identity(RPC_SCHEMA_VERSION, &local, &remote),
            Ok(())
        );
    }

    #[test]
    fn schema_mismatch_is_reported_first() {
        let local = build("2026.6.0", Some("abc123"), Some(false));
        // Different build *and* schema: schema wins.
        let remote = identity(
            RPC_SCHEMA_VERSION + 1,
            build("2026.7.0", Some("def456"), Some(false)),
        );
        assert_eq!(
            verify_daemon_identity(RPC_SCHEMA_VERSION, &local, &remote),
            Err(IdentityMismatch::Schema {
                local: RPC_SCHEMA_VERSION,
                remote: RPC_SCHEMA_VERSION + 1,
            })
        );
    }

    #[test]
    fn same_schema_different_commit_is_a_build_mismatch() {
        let local = build("2026.6.0", Some("abc123"), Some(false));
        let remote = identity(RPC_SCHEMA_VERSION, build("2026.6.0", Some("def456"), Some(false)));
        assert!(matches!(
            verify_daemon_identity(RPC_SCHEMA_VERSION, &local, &remote),
            Err(IdentityMismatch::Build { .. })
        ));
    }

    #[test]
    fn dirty_daemon_differs_from_clean_release_off_same_commit() {
        // The stale-shared-daemon footgun's cleaner cousin: same version and
        // commit, but one is a dev (dirty) build and the other a clean release.
        let local = build("2026.6.0", Some("abc123"), Some(false));
        let remote = identity(RPC_SCHEMA_VERSION, build("2026.6.0", Some("abc123"), Some(true)));
        assert!(matches!(
            verify_daemon_identity(RPC_SCHEMA_VERSION, &local, &remote),
            Err(IdentityMismatch::Build { .. })
        ));
    }

    #[test]
    fn separately_compiled_identical_builds_match() {
        // The CLI and MCP adapter are compiled independently; identical source
        // must fingerprint equal or they could never share a session.
        let one = build("2026.6.0", Some("abc123"), Some(true));
        let two = build("2026.6.0", Some("abc123"), Some(true));
        let remote = identity(RPC_SCHEMA_VERSION, two);
        assert_eq!(
            verify_daemon_identity(RPC_SCHEMA_VERSION, &one, &remote),
            Ok(())
        );
    }

    #[test]
    fn describe_renders_sha_and_dirty() {
        assert_eq!(
            build("2026.6.0", Some("abc123"), Some(true)).describe(),
            "2026.6.0 (abc123, dirty)"
        );
        assert_eq!(
            build("2026.6.0", Some("abc123"), Some(false)).describe(),
            "2026.6.0 (abc123)"
        );
        assert_eq!(build("2026.6.0", None, None).describe(), "2026.6.0");
    }
}
