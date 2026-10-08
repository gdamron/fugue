//! No module reads a number from its config with serde_json's raw
//! accessors: those default a float-spelled integer (`72.0`) and narrow
//! with `as`, which the reader exists to stop.

use super::registry::NOT_YET_MIGRATED;
use std::path::Path;

/// Module directories that may still call the raw accessors.
const ALLOWED: &[(&str, &str)] = &[(
    "control_scheduler",
    "reads its schedule and pacing values, not module config; FUG-302 owns them",
)];

const RAW_READS: [&str; 3] = [".as_u64()", ".as_i64()", ".as_f64()"];

/// The non-test lines of `source`: an inline `#[cfg(test)] mod … {` and
/// everything after it are left out.
fn non_test_lines(source: &str) -> impl Iterator<Item = (usize, &str)> {
    let lines: Vec<&str> = source.lines().collect();
    let end = lines
        .windows(2)
        .position(|pair| {
            pair[0].trim() == "#[cfg(test)]"
                && pair[1].trim_start().starts_with("mod ")
                && pair[1].trim_end().ends_with('{')
        })
        .unwrap_or(lines.len());
    lines.into_iter().take(end).enumerate()
}

/// Pushes each raw read in the non-test code at `path` (a file, or a
/// directory less its `tests` directories and `tests.rs` files).
fn scan(path: &Path, found: &mut Vec<String>) {
    let name = path.file_name().unwrap().to_string_lossy();
    if path.is_dir() {
        if name != "tests" {
            for entry in std::fs::read_dir(path).unwrap() {
                scan(&entry.unwrap().path(), found);
            }
        }
        return;
    }
    if name == "tests.rs" || !name.ends_with(".rs") {
        return;
    }
    let source = std::fs::read_to_string(path).unwrap();
    for (index, line) in non_test_lines(&source) {
        if RAW_READS.iter().any(|read| line.contains(read)) {
            let shown = path
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(path);
            found.push(format!(
                "{}:{}: {}",
                shown.display(),
                index + 1,
                line.trim()
            ));
        }
    }
}

#[test]
fn modules_read_config_numbers_only_through_the_reader() {
    let modules = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/modules");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&modules).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let allowed = ALLOWED.iter().any(|(dir, _)| *dir == name);
        if !allowed && !NOT_YET_MIGRATED.contains(&name.as_str()) {
            scan(&path, &mut found);
        }
    }
    assert!(
        found.is_empty(),
        "read these through module_config::ConfigReader (or whole_number / finite_f32):\n{}",
        found.join("\n")
    );
}

#[test]
fn the_guard_sees_raw_reads_but_not_inline_tests() {
    let source = "let a = v.as_u64();\n#[cfg(test)]\nmod tests {\n    v.as_f64();\n}\n";
    let lines: Vec<_> = non_test_lines(source).map(|(_, line)| line).collect();
    assert_eq!(lines, vec!["let a = v.as_u64();"]);
}
