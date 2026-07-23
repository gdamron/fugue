//! Captures a best-effort build fingerprint (git commit + working-tree
//! dirtiness) for the runtime daemon identity handshake.
//!
//! The fingerprint lets a connecting client tell a stale daemon apart from a
//! matching one even when the wire schema is unchanged — the exact footgun where
//! an old release daemon keeps the same `RPC_SCHEMA_VERSION` but has different
//! behavior (e.g. missing config keys). Git lookups are best-effort: outside a
//! checkout (a packaged crate) the values are simply absent and the daemon
//! reports only its crate version.
//!
//! Deliberately *no* build timestamp: the CLI and the MCP adapter are compiled
//! separately, so two binaries built from identical source must produce equal
//! fingerprints — otherwise they could never share a session. Identity is
//! therefore "same version + same commit + same dirtiness", all of which are
//! stable across independent compiles.

use std::process::Command;

fn main() {
    // Re-run whenever HEAD moves so a fresh commit refreshes the fingerprint.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=.git/HEAD");

    if let Some(sha) = git(&["rev-parse", "--short=12", "HEAD"]) {
        println!("cargo:rustc-env=FUGUE_GIT_SHA={sha}");
    }
    if let Some(status) = git(&["status", "--porcelain"]) {
        let dirty = if status.is_empty() { "0" } else { "1" };
        println!("cargo:rustc-env=FUGUE_GIT_DIRTY={dirty}");
    }
}

/// Runs a git subcommand, returning its trimmed stdout on success or `None` when
/// git is unavailable or the command fails (e.g. not a checkout).
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}
