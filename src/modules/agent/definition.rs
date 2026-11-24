//! An agent module's `definition`: an installed agent package it runs as.
//!
//! The reference (`id@requirement`) resolves as the module builds, on the
//! control thread building the graph, through
//! [`crate::pkg::resolve_agent`] against [`crate::pkg::default_packages_dir`]
//! and under the lockfile governing that thread. The resolved system prompt
//! becomes the module's `system_prompt`; the definition's default model is
//! used when the config names none. Nothing here reaches the audio thread:
//! the module holds the result in its control-side state only.

use serde_json::Value;

/// The config key naming the definition, and the read-only control that
/// shows it.
pub(super) const DEFINITION: &str = "definition";
/// The read-only control showing what the definition resolved to.
pub(super) const RESOLVED: &str = "resolved_definition";
/// The config key the definition's prompt replaces.
const SYSTEM_PROMPT: &str = "system_prompt";

/// What a module's definition resolved to, as the module runs with it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AgentDefinition {
    /// The reference as the config wrote it.
    pub(super) reference: String,
    /// The composed system prompt.
    pub(super) system_prompt: String,
    /// The definition's default model, if it names one.
    pub(super) model: Option<String>,
    /// The resolution as the `resolved_definition` control reads it.
    pub(super) summary: String,
}

impl AgentDefinition {
    /// The definition's default model, if it names one.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

/// The definition `config` names, resolved; `None` when it names none (no
/// key, or an empty one). Refuses a definition beside a non-empty inline
/// `system_prompt`, and one that does not resolve.
pub(super) fn from_config(config: &Value) -> Result<Option<AgentDefinition>, String> {
    let reference = match config.get(DEFINITION) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(reference)) if reference.trim().is_empty() => return Ok(None),
        Some(Value::String(reference)) => reference.trim(),
        Some(other) => {
            return Err(format!(
                "agent config '{DEFINITION}' must be an `id@requirement` string, got {other}"
            ))
        }
    };
    let inline = config.get(SYSTEM_PROMPT).and_then(Value::as_str);
    if inline.is_some_and(|prompt| !prompt.is_empty()) {
        return Err(format!(
            "agent config sets both '{DEFINITION}' and '{SYSTEM_PROMPT}': the definition \
             {reference} supplies the system prompt, so leave '{SYSTEM_PROMPT}' out"
        ));
    }
    resolve(reference).map(Some)
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve(reference: &str) -> Result<AgentDefinition, String> {
    let packages_dir = crate::pkg::default_packages_dir()
        .map_err(|err| format!("cannot resolve agent {reference}: {err}"))?;
    let agent =
        crate::pkg::resolve_agent(reference, &packages_dir).map_err(|err| err.to_string())?;
    Ok(AgentDefinition {
        reference: reference.to_string(),
        summary: summary(&agent).to_string(),
        system_prompt: agent.system_prompt,
        model: agent.model,
    })
}

#[cfg(target_arch = "wasm32")]
fn resolve(reference: &str) -> Result<AgentDefinition, String> {
    Err(format!(
        "cannot resolve agent {reference}: agent packages are not available on wasm32"
    ))
}

/// The resolution a person reads: the package, its default model and tools,
/// and the prompt's size and parts in prompt order.
#[cfg(not(target_arch = "wasm32"))]
fn summary(agent: &crate::pkg::ResolvedAgent) -> Value {
    use crate::pkg::PromptSource;
    use serde_json::json;
    let parts: Vec<Value> = (agent.parts.iter())
        .map(|part| {
            let source = match part.source {
                PromptSource::Persona => "persona",
                PromptSource::Policy => "policy",
                PromptSource::Skill => "skill",
            };
            json!({
                "source": source,
                "package": part.package,
                "version": part.version,
                "chars": part.chars,
            })
        })
        .collect();
    json!({
        "id": agent.id,
        "version": agent.version,
        "model": agent.model,
        "tools": agent.tools,
        "chars": agent.system_prompt.chars().count(),
        "parts": parts,
    })
}

/// Whether an authored write of `key` to an `agent` module with `config`
/// plays without being recorded in the document: a `system_prompt` written
/// over a definition's prompt, which the document cannot hold beside its
/// `definition`. It plays until the next reload restores the definition's.
pub(crate) fn plays_unrecorded(module_type: &str, config: &Value, key: &str) -> bool {
    unstated_keys(module_type, config).contains(&key)
}

/// The keys a module of `module_type` with `config` plays from elsewhere
/// whatever its config states: an agent naming a definition plays the
/// definition's `system_prompt` even beside an empty inline one, so a
/// reload restores a live write of it.
pub(crate) fn unstated_keys(module_type: &str, config: &Value) -> &'static [&'static str] {
    match names_definition(module_type, config) {
        true => &[SYSTEM_PROMPT],
        false => &[],
    }
}

/// Why an edit batch may not author `key` of `spec` into the document, if
/// it may not: a `system_prompt` over a definition's, which the document
/// cannot hold beside it. A live write plays it instead.
pub(crate) fn unauthorable(spec: &crate::ModuleSpec, key: &str) -> Option<String> {
    plays_unrecorded(&spec.module_type, &spec.config, key).then(|| {
        format!(
            "module '{}' runs the agent definition in its '{DEFINITION}', which supplies its \
             '{key}': an edit cannot author one beside it; write the control live to play \
             another until the next reload",
            spec.id
        )
    })
}

/// Whether a module of `module_type` with `config` is an agent naming a
/// definition, so holds what its packages composed.
pub(crate) fn names_definition(module_type: &str, config: &Value) -> bool {
    module_type == super::TYPE_ID
        && (config.get(DEFINITION).and_then(Value::as_str)).is_some_and(|r| !r.trim().is_empty())
}

#[cfg(test)]
#[path = "definition_tests.rs"]
mod tests;
