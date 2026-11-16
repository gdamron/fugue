//! Reads the daemon's side of the index through the discovery reads it
//! already answers, one request at a time, so a client drives it with its
//! own connection and a daemon needs nothing new.

use std::collections::VecDeque;

use super::entries::unread_type_entry;
use super::{content_entries, control_entries, module_type_entries, package_entries, Index};
use crate::pkg::content::ContentListQuery;
use crate::rpc::{ModuleTypeQuery, PackageListQuery, RpcCommand, TypeDetail};
use crate::{RpcRequestPayload, RpcResponsePayload};

/// The most module types one full description asks for. Core takes 64, but
/// a full description of 64 types nears its response cap.
const TYPES_PER_REQUEST: usize = 32;
/// Entries per content or package page, the most a page holds.
const PAGE_LIMIT: usize = 100;
/// The most pages read of each catalog; past it the answer says the rest
/// was not searched.
const MAX_PAGES: usize = 5;

/// Where a gather stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Snapshot,
    TypeNames,
    TypeDetails,
    Examples,
    Developments,
    Packages,
    Done,
}

impl Step {
    fn after(self) -> Self {
        match self {
            Step::Snapshot => Step::TypeNames,
            Step::TypeNames | Step::TypeDetails => Step::Examples,
            Step::Examples => Step::Developments,
            Step::Developments => Step::Packages,
            Step::Packages | Step::Done => Step::Done,
        }
    }

    fn source(self) -> &'static str {
        match self {
            Step::Snapshot => "The loaded piece",
            Step::TypeNames | Step::TypeDetails => "Module types",
            Step::Examples => "Examples",
            Step::Developments => "Developments",
            Step::Packages => "Packages",
            Step::Done => "Nothing",
        }
    }

    /// The tool that lists this source in full, with its diagnostics.
    fn lister(self) -> &'static str {
        match self {
            Step::Examples => "list_examples",
            Step::Developments => "list_developments",
            Step::Packages => "list_packages",
            _ => "describe_module_types",
        }
    }
}

/// Builds the daemon's side of an [`Index`]: the loaded piece's controls,
/// module types, examples, developments and installed packages.
///
/// ```ignore
/// let mut gather = Gather::new();
/// while let Some(request) = gather.next_request() {
///     gather.answer(daemon.request(request).map_err(|e| e.to_string()));
/// }
/// let mut index = gather.finish();
/// ```
///
/// A read that fails is noted in the index and skipped, so a search still
/// answers from everything else. A batch of type descriptions that fails is
/// asked again in halves, so one type that cannot be described costs only
/// its own description.
#[derive(Debug, Default)]
pub struct Gather {
    step: Option<Step>,
    index: Index,
    types: VecDeque<String>,
    asked_types: usize,
    /// The most types the next description asks for; 0 until the first.
    batch: usize,
    cursor: Option<String>,
    pages: usize,
}

impl Gather {
    /// A gather that has read nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    fn step(&self) -> Step {
        self.step.unwrap_or(Step::Snapshot)
    }

    /// The next request to send, or `None` once everything is read.
    pub fn next_request(&mut self) -> Option<RpcRequestPayload> {
        let command = match self.step() {
            Step::Snapshot => return Some(RpcRequestPayload::GetSnapshot),
            Step::TypeNames => RpcCommand::DescribeModuleTypes(ModuleTypeQuery::default()),
            Step::TypeDetails => {
                if self.batch == 0 {
                    self.batch = TYPES_PER_REQUEST;
                }
                self.asked_types = self.types.len().min(self.batch);
                RpcCommand::DescribeModuleTypes(ModuleTypeQuery {
                    types: Some(self.types.iter().take(self.asked_types).cloned().collect()),
                    detail: TypeDetail::Full,
                })
            }
            Step::Examples => RpcCommand::ListExamples {
                query: self.content_query(),
            },
            Step::Developments => RpcCommand::ListDevelopments {
                query: self.content_query(),
            },
            Step::Packages => RpcCommand::ListPackages {
                query: PackageListQuery {
                    limit: PAGE_LIMIT,
                    cursor: self.cursor.clone(),
                    ..PackageListQuery::default()
                },
            },
            Step::Done => return None,
        };
        Some(RpcRequestPayload::Command(command))
    }

    fn content_query(&self) -> ContentListQuery {
        ContentListQuery {
            limit: PAGE_LIMIT,
            cursor: self.cursor.clone(),
            ..ContentListQuery::default()
        }
    }

    /// Takes the daemon's reply to the last request, or why it failed.
    pub fn answer(&mut self, reply: Result<RpcResponsePayload, String>) {
        let step = self.step();
        let next_cursor = match (step, reply) {
            (Step::TypeDetails, Ok(RpcResponsePayload::Error(error))) => {
                return self.retry_types(&error.message)
            }
            (_, Err(error)) => return self.skip(step, &error),
            (_, Ok(RpcResponsePayload::Error(error))) => return self.skip(step, &error.message),
            (_, Ok(RpcResponsePayload::ContentError { error })) => {
                return self.skip(step, &error.to_string())
            }
            (Step::Snapshot, Ok(RpcResponsePayload::Snapshot(snapshot))) => {
                self.index.set_piece_loaded(snapshot.status.running);
                if snapshot.status.running {
                    self.index.extend(control_entries(&snapshot));
                }
                None
            }
            (Step::TypeNames, Ok(RpcResponsePayload::ModuleTypes { discovery })) => {
                self.types = discovery.index.types.into();
                self.step = Some(if self.types.is_empty() {
                    step.after()
                } else {
                    Step::TypeDetails
                });
                return;
            }
            (Step::TypeDetails, Ok(RpcResponsePayload::ModuleTypes { discovery })) => {
                self.index.extend(module_type_entries(&discovery));
                self.types.drain(..self.asked_types);
                if !self.types.is_empty() {
                    return;
                }
                None
            }
            (Step::Examples, Ok(RpcResponsePayload::Examples { catalog }))
            | (Step::Developments, Ok(RpcResponsePayload::Developments { catalog })) => {
                self.index.extend(content_entries(&catalog));
                self.note_left_out(
                    step,
                    catalog.diagnostics.len(),
                    catalog.diagnostics_truncated,
                );
                catalog.next_cursor
            }
            (Step::Packages, Ok(RpcResponsePayload::Packages(list))) => {
                self.index.extend(package_entries(&list));
                self.note_left_out(step, list.diagnostics.len(), list.diagnostics_truncated);
                list.next_cursor
            }
            (_, Ok(other)) => {
                return self.skip(step, &format!("unexpected reply {}", reply_name(&other)));
            }
        };
        self.pages += 1;
        match next_cursor {
            Some(cursor) if self.pages < MAX_PAGES => self.cursor = Some(cursor),
            Some(_) => {
                self.index.note(format!(
                    "{} past the first {} were not searched.",
                    step.source(),
                    MAX_PAGES * PAGE_LIMIT
                ));
                self.advance(step);
            }
            None => self.advance(step),
        }
    }

    /// Asks again for half of a batch of types the daemon refused (a batch
    /// can outgrow its response cap), or, for a type refused alone, gives it
    /// a card by name and goes on. A failed connection skips them all.
    fn retry_types(&mut self, error: &str) {
        if self.asked_types > 1 {
            self.batch = self.asked_types / 2;
            return;
        }
        self.batch = TYPES_PER_REQUEST;
        if let Some(name) = self.types.pop_front() {
            self.index.add(unread_type_entry(
                &name,
                &format!("Could not be described: {error}"),
            ));
        }
        if self.types.is_empty() {
            self.advance(Step::TypeDetails);
        }
    }

    /// Notes the entries a page reports it left out as unreadable.
    fn note_left_out(&mut self, step: Step, count: usize, truncated: bool) {
        if count == 0 && !truncated {
            return;
        }
        let count = match truncated {
            true => format!("more than {count}"),
            false => count.to_string(),
        };
        self.index.note(format!(
            "{}: {count} could not be read and were not searched; {} shows why.",
            step.source(),
            step.lister()
        ));
    }

    fn skip(&mut self, step: Step, error: &str) {
        self.index
            .note(format!("{} could not be read: {error}", step.source()));
        self.advance(step);
    }

    fn advance(&mut self, step: Step) {
        self.step = Some(step.after());
        self.cursor = None;
        self.pages = 0;
    }

    /// The index read so far.
    pub fn finish(self) -> Index {
        self.index
    }
}

/// The name of a reply's variant, for a note about one that was not
/// expected.
fn reply_name(reply: &RpcResponsePayload) -> String {
    match serde_json::to_value(reply) {
        Ok(value) => (value.get("kind").and_then(|kind| kind.as_str()))
            .unwrap_or("of another kind")
            .to_string(),
        Err(_) => "of another kind".to_string(),
    }
}

#[cfg(test)]
#[path = "gather_tests.rs"]
mod tests;
