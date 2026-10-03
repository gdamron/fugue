//! Applying one edit at a time to the candidate document.

use std::collections::{BTreeSet, HashMap};

use super::{CandidateWrite, EditFacts, EditedCandidate, ModuleFacts};
use crate::invention::authored_document;
use crate::invention::format::{Connection, Invention};
use crate::rpc::{non_finite_refusal, ControlWrite, EditFailureReason, StructuralEdit};
use crate::{ControlKind, ControlValue};

/// How many names a refusal lists when it shows what is available.
const LISTED_NAMES: usize = 24;

/// The most of a refused value a refusal echoes, in bytes.
const ECHOED_VALUE_BYTES: usize = 64;

/// The most of a module type's own config error a refusal carries, in
/// bytes. Larger than an echoed value because it is prose, but bounded,
/// since a module may echo the config it refused.
const MODULE_ERROR_BYTES: usize = 256;

/// Why an edit cannot apply; the caller adds its index and op.
pub(super) struct Refusal(pub(super) EditFailureReason, pub(super) String);

type Applied = Result<(), Refusal>;

pub(super) struct Candidate<'f, F> {
    document: Invention,
    facts: &'f mut F,
    /// Facts for modules this batch added, which the runtime does not know.
    added: HashMap<String, ModuleFacts>,
    control_writes: Vec<CandidateWrite>,
    named_modules: BTreeSet<String>,
    /// Ids in the document the batch started from.
    original: BTreeSet<String>,
    /// Ids from `original` that an edit removed.
    removed_originals: BTreeSet<String>,
}

impl<'f, F: EditFacts> Candidate<'f, F> {
    pub(super) fn new(document: Invention, facts: &'f mut F) -> Self {
        let original = document
            .modules
            .iter()
            .map(|spec| spec.id.clone())
            .collect();
        Self {
            document,
            facts,
            added: HashMap::new(),
            control_writes: Vec::new(),
            named_modules: BTreeSet::new(),
            original,
            removed_originals: BTreeSet::new(),
        }
    }

    pub(super) fn finish(self) -> EditedCandidate {
        let replaced = self
            .removed_originals
            .into_iter()
            .filter(|id| self.document.modules.iter().any(|spec| &spec.id == id))
            .collect();
        EditedCandidate {
            document: self.document,
            control_writes: self.control_writes,
            named_modules: self.named_modules,
            replaced,
        }
    }

    /// Applies `edit`, the batch's edit at `index`.
    pub(super) fn apply(&mut self, index: usize, edit: &StructuralEdit) -> Applied {
        match edit {
            StructuralEdit::AddModule {
                id,
                module_type,
                config,
            } => self.add_module(id, module_type, config),
            StructuralEdit::RemoveModule { id } => self.remove_module(id),
            StructuralEdit::Connect {
                from,
                from_port,
                to,
                to_port,
            } => self.connect(from, from_port, to, to_port),
            StructuralEdit::Disconnect {
                from,
                from_port,
                to,
                to_port,
            } => self.disconnect(from, from_port, to, to_port),
            StructuralEdit::SetControl {
                module_id,
                key,
                value,
            } => self.set_control(index, module_id, key, value),
        }
    }

    fn add_module(&mut self, id: &str, module_type: &str, config: &serde_json::Value) -> Applied {
        self.named_modules.insert(id.to_string());
        if self.exists(id) {
            return Err(Refusal(
                EditFailureReason::DuplicateModule,
                format!("module '{id}' already exists; remove it first to replace it"),
            ));
        }
        if !self.facts.has_type(module_type) {
            return Err(Refusal(
                EditFailureReason::UnknownModuleType,
                format!("unknown module type '{module_type}'"),
            ));
        }
        let facts = self
            .facts
            .describe(id, module_type, config)
            .map_err(|error| {
                Refusal(
                    EditFailureReason::InvalidConfig,
                    format!(
                        "module type '{module_type}' refused the config: {}",
                        bounded(&error, MODULE_ERROR_BYTES)
                    ),
                )
            })?;
        authored_document::upsert_module(&mut self.document, id, module_type, config);
        self.added.insert(id.to_string(), facts);
        Ok(())
    }

    fn remove_module(&mut self, id: &str) -> Applied {
        self.named_modules.insert(id.to_string());
        if !self.exists(id) {
            return Err(unknown_module(id));
        }
        authored_document::remove_module(&mut self.document, id);
        if self.original.contains(id) {
            self.removed_originals.insert(id.to_string());
        }
        // As the runtime does, a removed module takes its connections with it.
        self.document
            .connections
            .retain(|conn| conn.from != id && conn.to != id);
        if self.added.remove(id).is_some() {
            self.facts.forget(id);
        }
        // A write aimed at the instance being removed goes with it.
        self.control_writes
            .retain(|candidate| candidate.write.module_id != id);
        Ok(())
    }

    fn connect(&mut self, from: &str, from_port: &str, to: &str, to_port: &str) -> Applied {
        self.name_endpoints(from, to);
        let source = self.module_facts(from)?;
        if !source.outputs.iter().any(|port| port == from_port) {
            return Err(Refusal(
                EditFailureReason::UnknownPort,
                format!(
                    "module '{from}' has no output port '{from_port}' (available: {})",
                    listing(&source.outputs)
                ),
            ));
        }
        let dest = self.module_facts(to)?;
        if !dest.inputs.iter().any(|port| port == to_port) {
            return Err(Refusal(
                EditFailureReason::UnknownPort,
                format!(
                    "module '{to}' has no input port '{to_port}' (available: {})",
                    listing(&dest.inputs)
                ),
            ));
        }
        if self
            .document
            .connections
            .iter()
            .any(|conn| same_connection(conn, from, from_port, to, to_port))
        {
            return Err(Refusal(
                EditFailureReason::ConnectionExists,
                format!("{from}.{from_port} is already connected to {to}.{to_port}"),
            ));
        }
        self.document.connections.push(Connection {
            from: from.to_string(),
            from_port: Some(from_port.to_string()),
            to: to.to_string(),
            to_port: Some(to_port.to_string()),
        });
        Ok(())
    }

    fn disconnect(&mut self, from: &str, from_port: &str, to: &str, to_port: &str) -> Applied {
        self.name_endpoints(from, to);
        for id in [from, to] {
            if !self.exists(id) {
                return Err(unknown_module(id));
            }
        }
        let before = self.document.connections.len();
        self.document
            .connections
            .retain(|conn| !same_connection(conn, from, from_port, to, to_port));
        if self.document.connections.len() == before {
            return Err(Refusal(
                EditFailureReason::ConnectionNotFound,
                format!("{from}.{from_port} is not connected to {to}.{to_port}"),
            ));
        }
        Ok(())
    }

    fn set_control(
        &mut self,
        index: usize,
        module_id: &str,
        key: &str,
        value: &ControlValue,
    ) -> Applied {
        self.named_modules.insert(module_id.to_string());
        let facts = self.module_facts(module_id)?;
        let Some(kind) = facts.controls.get(key) else {
            let keys: Vec<String> = facts.controls.keys().cloned().collect();
            return Err(Refusal(
                EditFailureReason::UnknownControl,
                format!(
                    "module '{module_id}' has no control '{key}' (available: {})",
                    listing(&keys)
                ),
            ));
        };
        // The same coercion a standalone authored write applies.
        let applied = value.clone().coerced_to(kind);
        if !fits_kind(&applied, kind) {
            return Err(Refusal(
                EditFailureReason::InvalidControlValue,
                format!(
                    "control '{module_id}.{key}' expects {}, got {}",
                    kind_name(kind),
                    echo(value)
                ),
            ));
        }
        // Only a number control can hold a number once the value fits its
        // kind. A NaN or infinity (a wire number too large for an f32, or
        // text such as "inf") is refused as `check_edit_batch` refuses it.
        if let Some(message) = non_finite_refusal(module_id, key, &applied) {
            return Err(Refusal(EditFailureReason::InvalidControlValue, message));
        }
        authored_document::write_control(&mut self.document, module_id, key, &applied);
        self.control_writes.push(CandidateWrite {
            edit_index: index,
            write: ControlWrite::new(module_id, key, applied),
        });
        Ok(())
    }

    fn name_endpoints(&mut self, from: &str, to: &str) {
        self.named_modules.insert(from.to_string());
        self.named_modules.insert(to.to_string());
    }

    fn exists(&self, id: &str) -> bool {
        self.document.modules.iter().any(|spec| spec.id == id)
    }

    /// Facts for a module that exists at this point in the batch.
    fn module_facts(&self, id: &str) -> Result<ModuleFacts, Refusal> {
        if !self.exists(id) {
            return Err(unknown_module(id));
        }
        if let Some(facts) = self.added.get(id) {
            return Ok(facts.clone());
        }
        self.facts.module(id).ok_or_else(|| {
            Refusal(
                EditFailureReason::UnknownModule,
                format!("module '{id}' is in the document but is not running"),
            )
        })
    }
}

fn unknown_module(id: &str) -> Refusal {
    Refusal(
        EditFailureReason::UnknownModule,
        format!("no module '{id}' exists at this point in the batch"),
    )
}

/// Matches the way the retained document spells a connection: a missing
/// port name is the empty port, as `RuntimeState::document` writes it.
fn same_connection(
    conn: &Connection,
    from: &str,
    from_port: &str,
    to: &str,
    to_port: &str,
) -> bool {
    conn.from == from
        && conn.to == to
        && conn.from_port.as_deref().unwrap_or("") == from_port
        && conn.to_port.as_deref().unwrap_or("") == to_port
}

/// Whether a coerced value has the shape its control declares. Ranges and
/// option lists are left to the module, which may clamp or accept aliases;
/// at commit, `validate_control` refuses whatever the module's setter would
/// refuse.
fn fits_kind(value: &ControlValue, kind: &ControlKind) -> bool {
    matches!(
        (value, kind),
        (ControlValue::Number(_), ControlKind::Number { .. })
            | (ControlValue::Bool(_), ControlKind::Bool)
            | (ControlValue::String(_), ControlKind::String { .. })
    )
}

fn kind_name(kind: &ControlKind) -> &'static str {
    match kind {
        ControlKind::Number { .. } => "a number",
        ControlKind::Bool => "a boolean",
        ControlKind::String { .. } => "a string",
    }
}

/// A refused value as a message shows it, cut to about
/// [`ECHOED_VALUE_BYTES`].
fn echo(value: &ControlValue) -> String {
    match value {
        ControlValue::Number(number) => number.to_string(),
        ControlValue::Bool(flag) => flag.to_string(),
        ControlValue::String(text) => format!("{:?}", bounded(text, ECHOED_VALUE_BYTES)),
    }
}

/// `text` cut at a character boundary to at most `max` bytes, marked with
/// `…` when cut.
fn bounded(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn listing(names: &[String]) -> String {
    if names.is_empty() {
        return "none".to_string();
    }
    let shown = names.iter().take(LISTED_NAMES).cloned().collect::<Vec<_>>();
    let more = names.len().saturating_sub(LISTED_NAMES);
    if more == 0 {
        shown.join(", ")
    } else {
        format!("{}, and {more} more", shown.join(", "))
    }
}
