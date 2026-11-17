//! Test-only: a stand-in for the demo agent package `fugue.demo.pulse-agent`
//! and the skill and policies it composes, installed into a packages dir as
//! `fyug install` lays them out.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

pub(crate) const PULSE_AGENT: &str = "fugue.demo.pulse-agent";
pub(crate) const PULSE_SKILL: &str = "fugue.demo.pulse-skill";
pub(crate) const CLEAN_AUDIO: &str = "fugue.policy.clean-audio";
pub(crate) const MUSICAL_INTENT: &str = "fugue.policy.musical-intent";
pub(crate) const PERSONA: &str = "Create concise Fugue pulse studies.";

/// Installs `id@version` under `packs`: its manifest, with `extra` fields,
/// and `files`.
pub(crate) fn install(
    packs: &Path,
    (id, version, kind): (&str, &str, &str),
    extra: Value,
    files: &[(&str, &str)],
) {
    let entry = match kind {
        "agent" => json!({ "definition": "agent.json" }),
        "skill" => json!({ "skill": "SKILL.md" }),
        _ => json!({ "policy": "POLICY.md" }),
    };
    let mut manifest = json!({
        "id": id, "version": version, "kind": kind, "license": "MIT",
        "authors": [{ "name": "Test" }], "targets": ["in-graph-agent"], "entry": entry,
    });
    for (key, value) in extra.as_object().unwrap() {
        manifest[key] = value.clone();
    }
    let dir = packs.join(id).join(version);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("fugue.pkg.json"), manifest.to_string()).unwrap();
    for (name, text) in files {
        fs::write(dir.join(name), text).unwrap();
    }
}

/// Installs the pulse agent 0.1.0 with `agent_json` as its definition file.
pub(crate) fn install_pulse_agent_as(packs: &Path, agent_json: &str) {
    let policies = json!([
        format!("{CLEAN_AUDIO}@^2026.11.0"),
        format!("{MUSICAL_INTENT}@^2026.11.0")
    ]);
    install(
        packs,
        (CLEAN_AUDIO, "2026.11.0", "policy"),
        json!({}),
        &[("POLICY.md", "Keep the audio clean.\n")],
    );
    install(
        packs,
        (MUSICAL_INTENT, "2026.11.0", "policy"),
        json!({}),
        &[("POLICY.md", "Serve the musical intent.")],
    );
    let skill =
        "---\nname: pulse\ndescription: Shape a pulse.\n---\n\nTurn a pulse into a phrase.\n";
    install(
        packs,
        (PULSE_SKILL, "0.1.0", "skill"),
        json!({ "policies": policies }),
        &[("SKILL.md", skill)],
    );
    install(
        packs,
        (PULSE_AGENT, "0.1.0", "agent"),
        json!({ "policies": policies, "skills": [format!("{PULSE_SKILL}@^0.1.0")] }),
        &[("agent.json", agent_json)],
    );
}

/// Installs the pulse agent 0.1.0 as the demo pack defines it: a persona
/// and default tools, no default model.
pub(crate) fn install_pulse_agent(packs: &Path) {
    let definition = json!({ "system_prompt": PERSONA, "default_tools": ["fugue.set_control"] });
    install_pulse_agent_as(packs, &definition.to_string());
}
