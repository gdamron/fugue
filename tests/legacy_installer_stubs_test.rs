//! The retired `install.sh` and `install.ps1` at the repository root only point
//! users at the product installer. Each must print that install command, exit
//! non-zero, write no files and make no network request, because people still
//! run the old one-line commands:
//!
//! ```text
//! curl -fsSL https://raw.githubusercontent.com/gdamron/fugue/main/install.sh | sh
//! powershell -ExecutionPolicy ByPass -c "irm https://raw.githubusercontent.com/gdamron/fugue/main/install.ps1 | iex"
//! ```
//!
//! The shell stub runs on Unix. The PowerShell stub runs under `pwsh`
//! everywhere it is installed and, on Windows, under Windows PowerShell 5.1
//! (`powershell`). A missing PowerShell skips its test with a message unless
//! `FUGUE_REQUIRE_POWERSHELL` is set, as it is where CI must exercise it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The product's documented one-line installers, as printed by the stubs.
const SH_COMMAND: &str =
    "curl -fsSL https://github.com/Ilusiv-media/fyug/releases/latest/download/install.sh | sh";
const PS_COMMAND: &str = "powershell -ExecutionPolicy ByPass -c \"irm https://github.com/Ilusiv-media/fyug/releases/latest/download/install.ps1 | iex\"";

fn repo_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name)
}

/// Scratch HOME, working directory and temp directory, so a stub that writes
/// anywhere it normally could shows up as a changed tree. `netlog` collects a
/// line for every fake network tool a stub runs.
struct Scratch {
    root: tempfile::TempDir,
    /// Where PowerShell keeps its own caches (startup profile data, modules),
    /// outside the compared tree: they are the host's writes, not the stub's.
    host_state: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("scratch dir");
        for dir in ["home", "work", "tmp", "fakebin", "netlog"] {
            fs::create_dir(root.path().join(dir)).expect("scratch subdir");
        }
        let host_state = tempfile::tempdir().expect("host state dir");
        Scratch { root, host_state }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn netlog_calls(&self) -> PathBuf {
        self.path("netlog").join("calls")
    }

    /// Every path under the scratch root with its size, sorted.
    fn snapshot(&self) -> Vec<String> {
        let mut entries = Vec::new();
        walk(self.root.path(), self.root.path(), &mut entries);
        entries.sort();
        entries
    }

    /// Points the child's home and temp locations at the scratch dirs.
    fn confine(&self, command: &mut Command) {
        command
            .current_dir(self.path("work"))
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("home"))
            .env("TMPDIR", self.path("tmp"))
            .env("TMP", self.path("tmp"))
            .env("TEMP", self.path("tmp"))
            .env("XDG_CACHE_HOME", self.host_state.path().join("cache"))
            .env("XDG_DATA_HOME", self.host_state.path().join("data"))
            .env("XDG_CONFIG_HOME", self.host_state.path().join("config"))
            .env("POWERSHELL_TELEMETRY_OPTOUT", "1")
            .env("POWERSHELL_UPDATECHECK", "Off")
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1");
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("read scratch dir") {
        let entry = entry.expect("scratch entry");
        let meta = fs::symlink_metadata(entry.path()).expect("scratch metadata");
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap()
            .display()
            .to_string();
        if meta.is_dir() {
            out.push(format!("{rel}/"));
            walk(root, &entry.path(), out);
        } else {
            out.push(format!("{rel} ({} bytes)", meta.len()));
        }
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// Asserts a stub failed with an ordinary non-zero exit (not a signal), that
/// a line of its output is exactly the install command, and that nothing in
/// the scratch tree changed.
fn assert_refuses(
    label: &str,
    output: &Output,
    command: &str,
    scratch: &Scratch,
    before: &[String],
) {
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    let code = output.status.code();
    assert!(
        matches!(code, Some(c) if c != 0),
        "{label}: expected a non-zero exit code, got {code:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
    let printed = stdout
        .lines()
        .chain(stderr.lines())
        .any(|l| l.trim() == command);
    assert!(
        printed,
        "{label}: no output line equals the install command {command:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(
        before,
        scratch.snapshot().as_slice(),
        "{label}: wrote files"
    );
    assert!(
        !scratch.netlog_calls().exists(),
        "{label}: ran a network tool: {}",
        fs::read_to_string(scratch.netlog_calls()).unwrap_or_default()
    );
}

/// The script with its printed command and comment lines removed: what is
/// left is the code that would run.
fn executable_text(script: &str, command: &str) -> String {
    script
        .replace(command, "")
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Tools and cmdlets that reach the network, plus any URL left outside the
/// printed command.
fn network_uses(code: &str) -> Vec<&'static str> {
    const FORBIDDEN: &[&str] = &[
        "curl",
        "wget",
        "nc",
        "ncat",
        "netcat",
        "telnet",
        "ssh",
        "scp",
        "ftp",
        "git",
        "Invoke-WebRequest",
        "iwr",
        "Invoke-RestMethod",
        "irm",
        "WebClient",
        "HttpClient",
        "WebRequest",
        "Start-BitsTransfer",
        "Test-Connection",
        "Resolve-DnsName",
        "TcpClient",
        "/dev/tcp",
        "http://",
        "https://",
    ];
    let lower = code.to_lowercase();
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    FORBIDDEN
        .iter()
        .copied()
        .filter(|needle| {
            let needle = needle.to_lowercase();
            lower.match_indices(&needle).any(|(at, _)| {
                let before = lower[..at].chars().next_back();
                let after = lower[at + needle.len()..].chars().next();
                let edge_ok = !needle.starts_with(is_word) || !before.is_some_and(is_word);
                let end_ok = !needle.ends_with(is_word) || !after.is_some_and(is_word);
                edge_ok && end_ok
            })
        })
        .collect()
}

fn read_stub(name: &str) -> String {
    fs::read_to_string(repo_file(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn stubs_name_the_product_installers() {
    assert!(read_stub("install.sh")
        .lines()
        .any(|l| l.contains(SH_COMMAND)));
    assert!(read_stub("install.ps1")
        .lines()
        .any(|l| l.contains(PS_COMMAND)));
}

#[test]
fn stub_code_has_no_network_client() {
    for (name, command) in [("install.sh", SH_COMMAND), ("install.ps1", PS_COMMAND)] {
        let code = executable_text(&read_stub(name), command);
        assert_eq!(
            network_uses(&code),
            Vec::<&str>::new(),
            "{name} reaches the network:\n{code}"
        );
    }
}

#[test]
fn network_scan_flags_clients_but_not_prose_or_the_printed_command() {
    assert_eq!(network_uses("echo hi | curl -T- host"), vec!["curl"]);
    assert_eq!(
        network_uses("iwr $u; Invoke-WebRequest $u"),
        vec!["Invoke-WebRequest", "iwr"]
    );
    assert_eq!(
        network_uses("echo see https://example.com"),
        vec!["https://"]
    );
    assert_eq!(
        network_uses("printf 'Nothing happened: syncing'"),
        Vec::<&str>::new()
    );
    let script = format!("# curl in a comment\nprintf '%s' '{SH_COMMAND}'\nexit 1\n");
    assert_eq!(
        network_uses(&executable_text(&script, SH_COMMAND)),
        Vec::<&str>::new()
    );
}

#[cfg(unix)]
mod shell {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    /// Fake network tools first on PATH; each records that it was run.
    fn install_fake_network_tools(scratch: &Scratch) {
        for tool in ["curl", "wget", "nc", "ssh", "git"] {
            let path = scratch.path("fakebin").join(tool);
            let body = format!(
                "#!/bin/sh\necho \"{tool} $*\" >> '{}'\nexit 0\n",
                scratch.netlog_calls().display()
            );
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn sh(scratch: &Scratch) -> Command {
        let mut command = Command::new("sh");
        command.env_clear().env(
            "PATH",
            format!("{}:/usr/bin:/bin", scratch.path("fakebin").display()),
        );
        scratch.confine(&mut command);
        command
    }

    /// `curl -fsSL <url> | sh`: the script arrives on standard input.
    #[test]
    fn piped_to_sh_prints_the_command_and_fails_without_side_effects() {
        let scratch = Scratch::new();
        install_fake_network_tools(&scratch);
        let before = scratch.snapshot();

        let mut child = sh(&scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sh");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(read_stub("install.sh").as_bytes())
            .expect("pipe the script");
        let output = child.wait_with_output().expect("wait for sh");

        assert_refuses("curl | sh", &output, SH_COMMAND, &scratch, &before);
    }

    #[test]
    fn run_as_a_file_prints_the_command_and_fails_without_side_effects() {
        let scratch = Scratch::new();
        install_fake_network_tools(&scratch);
        let before = scratch.snapshot();

        let output = sh(&scratch)
            .arg(repo_file("install.sh"))
            .stdin(Stdio::null())
            .output()
            .expect("run sh");

        assert_refuses("sh install.sh", &output, SH_COMMAND, &scratch, &before);
    }
}

/// Runs `exe` quietly, returning its trimmed output when it succeeds.
fn probe(exe: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(exe).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| text(&output.stdout).trim().to_string())
}

/// Runs the PowerShell stub under `exe` both as a file and the way the old
/// command ran it, piped into `iex`, each from a scratch HOME and directory.
fn check_powershell_stub(exe: &str, major_version: fn(u32) -> bool) {
    let version = probe(
        exe,
        &[
            "-NoProfile",
            "-Command",
            "$PSVersionTable.PSVersion.ToString()",
        ],
    );
    let Some(version) = version else {
        assert!(
            std::env::var_os("FUGUE_REQUIRE_POWERSHELL").is_none(),
            "{exe} is required (FUGUE_REQUIRE_POWERSHELL is set) but did not start"
        );
        eprintln!("skipping: {exe} is not installed");
        return;
    };
    let major: u32 = version.split('.').next().unwrap().parse().expect("version");
    assert!(
        major_version(major),
        "{exe} is PowerShell {version}, not the expected edition"
    );

    let script = repo_file("install.ps1");
    let script_path = script.display().to_string();
    let piped = format!("Get-Content -Raw -LiteralPath '{script_path}' | Invoke-Expression");
    let forms: [(&str, Vec<&str>); 2] = [
        (
            "-File",
            vec!["-ExecutionPolicy", "Bypass", "-File", &script_path],
        ),
        ("| iex", vec!["-Command", &piped]),
    ];

    for (form, args) in forms {
        let scratch = Scratch::new();
        let run = |extra: &[&str]| {
            let mut command = Command::new(exe);
            command
                .args(["-NoProfile", "-NonInteractive"])
                .args(extra)
                .stdin(Stdio::null());
            scratch.confine(&mut command);
            command.output().expect("run PowerShell")
        };
        // Start PowerShell once first so any first-run files it writes under
        // HOME are not blamed on the stub.
        run(&["-Command", "$null"]);
        let before = scratch.snapshot();

        let output = run(&args);
        assert_refuses(
            &format!("{exe} {form} (PowerShell {version})"),
            &output,
            PS_COMMAND,
            &scratch,
            &before,
        );
    }
}

#[test]
fn pwsh_stub_prints_the_command_and_fails_without_side_effects() {
    check_powershell_stub("pwsh", |major| major >= 7);
}

#[cfg(windows)]
#[test]
fn windows_powershell_stub_prints_the_command_and_fails_without_side_effects() {
    check_powershell_stub("powershell", |major| major == 5);
}
