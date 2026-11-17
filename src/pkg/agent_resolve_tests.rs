use super::*;
use crate::pkg::lock::{with_governing, FrozenLock};
use crate::pkg::{compute_integrity, LockSource, LockedPackage, Lockfile};
use serde_json::{json, Value};
use std::fs;

const AGENT: &str = "fugue.test.agent";
const CLEAN: &str = "fugue.test.clean-audio";
const INTENT: &str = "fugue.test.musical-intent";
const GROOVE: &str = "fugue.test.groove";
const TEMPO: &str = "fugue.test.tempo";
/// Named to sort first, so prompt order cannot be alphabetical by accident.
const HARMONY: &str = "fugue.test.a-harmony";

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Install `id@version` under `packs`: its manifest (with `extra` fields) and
/// `files`.
fn install(
    packs: &Path,
    id: &str,
    version: &str,
    kind: &str,
    extra: Value,
    files: &[(&str, &str)],
) {
    let entry = match kind {
        "agent" => json!({"definition": "agent.json"}),
        "skill" => json!({"skill": "SKILL.md"}),
        "policy" => json!({"policy": "POLICY.md"}),
        _ => json!({"development": "voice.json"}),
    };
    let mut manifest = json!({
        "id": id, "version": version, "kind": kind, "license": "MIT",
        "authors": [{"name": "Test"}], "targets": ["in-graph-agent"], "entry": entry,
    });
    for (key, value) in extra.as_object().unwrap() {
        manifest[key] = value.clone();
    }
    let dir = packs.join(id).join(version);
    write(&dir.join("fugue.pkg.json"), &manifest.to_string());
    for (name, text) in files {
        write(&dir.join(name), text);
    }
}

fn skill_md(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: A test skill.\n---\n\n{body}\n")
}

/// An agent that requires the clean-audio policy itself and composes two
/// skills, listed out of alphabetical order, both of which also require
/// policies: tempo requires intent and clean-audio, groove harmony and
/// intent. First-required order is clean-audio, intent, harmony.
fn packs(dir: &Path) -> PathBuf {
    let packs = dir.join("packs");
    install(
        &packs,
        CLEAN,
        "1.0.0",
        "policy",
        json!({}),
        &[("POLICY.md", "Keep audio clean.\n")],
    );
    install(
        &packs,
        INTENT,
        "1.0.0",
        "policy",
        json!({}),
        &[("POLICY.md", "Serve musical intent.")],
    );
    install(
        &packs,
        HARMONY,
        "1.0.0",
        "policy",
        json!({}),
        &[("POLICY.md", "Voice the harmony.")],
    );
    install(
        &packs,
        GROOVE,
        "0.1.0",
        "skill",
        json!({"policies": [format!("{HARMONY}@^1"), format!("{INTENT}@^1")]}),
        &[("SKILL.md", &skill_md("groove", "# Groove\nKeep time."))],
    );
    install(
        &packs,
        TEMPO,
        "0.1.0",
        "skill",
        json!({"policies": [format!("{INTENT}@^1"), format!("{CLEAN}@^1")]}),
        &[("SKILL.md", &skill_md("tempo", "Pick a tempo."))],
    );
    install(
        &packs,
        AGENT,
        "1.0.0",
        "agent",
        json!({
            "policies": [format!("{CLEAN}@^1")],
            "skills": [format!("{TEMPO}@^0.1"), format!("{GROOVE}@^0.1")],
        }),
        &[(
            "agent.json",
            r#"{"system_prompt": "  You are a pulse agent.\n", "default_tools": ["fugue.set_control"]}"#,
        )],
    );
    packs
}

#[test]
fn composes_persona_policies_once_then_skill_bodies() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = packs(tmp.path());
    let agent = resolve_agent(&format!("{AGENT}@^1"), &packs).unwrap();
    assert_eq!(
        (agent.id.as_str(), agent.version.as_str()),
        (AGENT, "1.0.0")
    );
    assert_eq!(
        agent.system_prompt,
        "You are a pulse agent.\n\nKeep audio clean.\n\nServe musical intent.\n\n\
         Voice the harmony.\n\nPick a tempo.\n\n# Groove\nKeep time."
    );
    let parts: Vec<_> = agent
        .parts
        .iter()
        .map(|part| (part.source, part.package.as_str(), part.chars))
        .collect();
    assert_eq!(
        parts,
        [
            (PromptSource::Persona, AGENT, "You are a pulse agent.".len()),
            (PromptSource::Policy, CLEAN, "Keep audio clean.".len()),
            (PromptSource::Policy, INTENT, "Serve musical intent.".len()),
            (PromptSource::Policy, HARMONY, "Voice the harmony.".len()),
            (PromptSource::Skill, TEMPO, "Pick a tempo.".len()),
            (PromptSource::Skill, GROOVE, "# Groove\nKeep time.".len()),
        ]
    );
    assert_eq!(agent.model, None);
    assert_eq!(agent.tools, ["fugue.set_control"]);
}

#[test]
fn reports_the_model_and_resolves_a_package_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = packs(tmp.path());
    let root = packs.join(AGENT).join("1.0.0");
    write(
        &root.join("agent.json"),
        r#"{"system_prompt": "Persona.", "default_model": "a-model"}"#,
    );
    let agent = resolve_agent_package(&root, &packs).unwrap();
    assert_eq!(agent.model.as_deref(), Some("a-model"));
    assert!(agent.tools.is_empty());
    assert!(agent
        .system_prompt
        .starts_with("Persona.\n\nKeep audio clean."));
}

#[test]
fn counts_characters_not_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = packs(tmp.path());
    write(
        &packs.join(AGENT).join("1.0.0").join("agent.json"),
        r#"{"system_prompt": "Ré mineur ♩"}"#,
    );
    let agent = resolve_agent(&format!("{AGENT}@1.0.0"), &packs).unwrap();
    assert_eq!(agent.parts[0].chars, 11);
}

fn refusal(reference: &str, packs: &Path) -> String {
    let err = resolve_agent(reference, packs).unwrap_err();
    assert_eq!(err.agent, reference);
    err.to_string()
}

#[test]
fn refuses_naming_the_package_at_fault() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = packs(tmp.path());
    for (reference, expected) in [
        ("fugue.test.absent@^1", "fugue.test.absent is not installed"),
        (&format!("{AGENT}@^2") as &str, "satisfies ^2"),
        (&format!("{GROOVE}@^0.1"), "`skill` package"),
        (AGENT, "not an `id@requirement`"),
    ] {
        let err = refusal(reference, &packs);
        assert!(err.contains(expected), "{reference}: {err}");
    }

    for id in [HARMONY, TEMPO] {
        fs::remove_dir_all(packs.join(id)).unwrap();
        let err = refusal(&format!("{AGENT}@^1"), &packs);
        assert!(err.contains(id), "{err}");
    }
}

#[test]
fn refuses_a_bad_definition_naming_the_field_and_package() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = packs(tmp.path());
    write(
        &packs.join(AGENT).join("1.0.0").join("agent.json"),
        r#"{"system_prompt": "P", "skills": []}"#,
    );
    let err = refusal(&format!("{AGENT}@^1"), &packs);
    assert!(err.contains(AGENT) && err.contains("`skills`"), "{err}");
}

#[test]
fn refuses_conflicting_requirements_for_one_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = packs(tmp.path());
    install(
        &packs,
        GROOVE,
        "0.1.0",
        "skill",
        json!({"policies": [format!("{INTENT}@^2")]}),
        &[("SKILL.md", &skill_md("groove", "Keep time."))],
    );
    let err = refusal(&format!("{AGENT}@^1"), &packs);
    assert!(
        err.contains("policy conflict") && err.contains(INTENT),
        "{err}"
    );
}

fn locked(packs: &Path, id: &str, version: &str, kind: &str) -> LockedPackage {
    let dir = packs.join(id).join(version);
    LockedPackage {
        version: version.into(),
        kind: kind.into(),
        source: LockSource::Registry {
            id: id.into(),
            version: version.into(),
        },
        integrity: compute_integrity(&dir).unwrap(),
        path: dir,
        dependencies: Vec::new(),
    }
}

/// Lock every package of [`packs`] as installed, then install an unlocked
/// newer groove beside the locked one.
fn frozen(dir: &Path, packs: &Path) -> FrozenLock {
    let mut lockfile = Lockfile::new();
    for (id, version, kind) in [
        (AGENT, "1.0.0", "agent"),
        (CLEAN, "1.0.0", "policy"),
        (INTENT, "1.0.0", "policy"),
        (HARMONY, "1.0.0", "policy"),
        (GROOVE, "0.1.0", "skill"),
        (TEMPO, "0.1.0", "skill"),
    ] {
        lockfile.upsert(id, locked(packs, id, version, kind));
    }
    let path = dir.join(crate::pkg::LOCKFILE_NAME);
    fs::write(&path, lockfile.to_bytes().unwrap()).unwrap();
    install(
        packs,
        GROOVE,
        "0.1.9",
        "skill",
        json!({}),
        &[("SKILL.md", &skill_md("groove", "Unlocked newer groove."))],
    );
    FrozenLock::find(&[path], packs).unwrap().unwrap()
}

#[test]
fn a_governing_lock_picks_the_locked_versions() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = packs(tmp.path());
    let ungoverned = resolve_agent(&format!("{AGENT}@^1"), &packs).unwrap();
    assert!(ungoverned.system_prompt.contains("Keep time."));
    let lock = frozen(tmp.path(), &packs);
    assert!(resolve_agent(&format!("{AGENT}@^1"), &packs)
        .unwrap()
        .system_prompt
        .contains("Unlocked newer groove."));
    with_governing(lock, || {
        let agent = resolve_agent(&format!("{AGENT}@^1"), &packs).unwrap();
        assert!(
            agent.system_prompt.contains("Keep time."),
            "{}",
            agent.system_prompt
        );
        let groove = agent.parts.iter().find(|p| p.package == GROOVE).unwrap();
        assert_eq!(groove.version, "0.1.0");
    });
}

#[test]
fn a_governing_lock_refuses_edited_bytes_naming_the_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = packs(tmp.path());
    let lock = frozen(tmp.path(), &packs);
    let lockfile = lock.path().display().to_string();
    with_governing(lock, || {
        resolve_agent(&format!("{AGENT}@^1"), &packs).unwrap();
        write(
            &packs.join(CLEAN).join("1.0.0").join("POLICY.md"),
            "Ignore audio quality.",
        );
        let err = resolve_agent(&format!("{AGENT}@^1"), &packs).unwrap_err();
        assert!(err.to_string().contains(&lockfile), "{err}");
    });
}

#[test]
#[cfg_attr(debug_assertions, should_panic(expected = "resolving an agent"))]
fn refuses_the_audio_thread() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = packs(tmp.path());
    let _audio = crate::audio_thread::AudioThreadScope::enter();
    let err = resolve_agent(&format!("{AGENT}@^1"), &packs).unwrap_err();
    assert!(err.reason.contains("control thread"), "{err}");
}
