//! `move_modules` inside a development: the arrangement saved in the
//! development's own document, the piece's (for an inline definition) or
//! its file, and never in an installed package's. Presentation only, as a
//! move of the piece's own modules: nothing is built, published or heard,
//! and it all runs on the calling control thread.

use std::sync::Arc;

use crate::invention::development_file::{self, NotWritten};
use crate::invention::layout::{Layout, Position};
use crate::invention::reload::{DevelopmentDefinitions, DevelopmentHome, LoadedDevelopments};
use crate::invention::runtime::RunningInvention;
use crate::rpc::{
    DevelopmentSavedIn, EditFailure, EditFailureReason, EditOp, RpcError, RpcErrorCode,
    StructuralEdit,
};
use crate::Invention;

/// The moves a batch makes inside one development, with what it starts
/// from: its layout as loaded.
pub(super) struct Arrangement {
    /// The batch index of the first edit that moves inside it.
    index: usize,
    home: DevelopmentHome,
    base: Layout,
    moves: Vec<(String, Option<Position>)>,
}

impl Arrangement {
    /// `base` (by default its own) with its moves made, in batch order.
    fn layout(&self, base: Option<&Layout>) -> Layout {
        let mut layout = base.unwrap_or(&self.base).clone();
        for (id, position) in &self.moves {
            match position {
                Some(position) => layout.modules.insert(id.clone(), *position),
                None => layout.modules.remove(id),
            };
        }
        layout
    }
}

/// The batch-level refusal for a move inside a development made with a
/// structural edit, or timed: it applies at once, with moves and names.
pub(super) fn refuse_mixed(edits: &[StructuralEdit], timed: bool) -> Result<(), RpcError> {
    if !edits.iter().any(|edit| edit.development().is_some()) {
        return Ok(());
    }
    Err(RpcError::new(
        RpcErrorCode::InvalidRequest,
        format!(
            "a move inside a development applies at once, in a batch of move_modules and \
             name_modules alone{}; nothing was applied",
            if timed { ", never timed" } else { "" }
        ),
    ))
}

/// The refusal of a development move at edit `index`.
fn refused(index: usize, reason: EditFailureReason, message: String) -> RpcError {
    RpcError::invalid_edit(EditFailure::new(
        index,
        EditOp::MoveModules,
        reason,
        message,
    ))
}

/// Where a move inside the development at `home` is saved.
pub(crate) fn saved_in(home: &DevelopmentHome) -> DevelopmentSavedIn {
    match home {
        DevelopmentHome::Piece(_) => DevelopmentSavedIn::Piece,
        DevelopmentHome::File { path, .. } => match development_file::never_written(path) {
            Some(reason) => DevelopmentSavedIn::ReadOnly { reason },
            None => DevelopmentSavedIn::File {
                path: path.display().to_string(),
            },
        },
        DevelopmentHome::Package => DevelopmentSavedIn::ReadOnly {
            reason: "it comes from an installed package".to_string(),
        },
    }
}

impl RunningInvention {
    /// The development moves in `edits`, one arrangement per development,
    /// each checked: the development exists and is saved somewhere it may
    /// be written, and each id is one of its own modules. At most one file
    /// is written per batch.
    pub(super) fn arrangements(
        &self,
        edits: &[StructuralEdit],
    ) -> Result<Vec<Arrangement>, RpcError> {
        let definitions = self.definitions();
        let loaded = definitions.loaded();
        let mut found: Vec<Arrangement> = Vec::new();
        for (index, edit) in edits.iter().enumerate() {
            let (Some(path), StructuralEdit::MoveModules { positions, .. }) =
                (edit.development(), edit)
            else {
                continue;
            };
            let named = path.join("/");
            let entry = loaded.find(path).map_err(|error| {
                refused(index, EditFailureReason::UnknownDevelopment, error.message)
            })?;
            if let DevelopmentSavedIn::ReadOnly { reason } = saved_in(&entry.home) {
                return Err(refused(
                    index,
                    EditFailureReason::ReadOnlyDevelopment,
                    format!(
                        "development '{named}' is never written here: {reason}; its \
                         arrangement stays as it is"
                    ),
                ));
            }
            let modules = &entry.definition.modules;
            if let Some(id) = (positions.keys()).find(|id| !modules.iter().any(|m| &m.id == *id)) {
                return Err(refused(
                    index,
                    EditFailureReason::UnknownModule,
                    format!("development '{named}' has no module '{id}'"),
                ));
            }
            let moves = positions.iter().map(|(id, at)| (id.clone(), *at));
            match found.iter_mut().find(|known| known.home == entry.home) {
                Some(known) => known.moves.extend(moves),
                None => found.push(Arrangement {
                    index,
                    home: entry.home.clone(),
                    base: entry.definition.layout.clone(),
                    moves: moves.collect(),
                }),
            }
        }
        let files = found
            .iter()
            .filter(|a| matches!(a.home, DevelopmentHome::File { .. }));
        if files.count() > 1 {
            return Err(RpcError::new(
                RpcErrorCode::InvalidRequest,
                "one apply_edits batch arranges developments in at most one file; \
                 nothing was applied",
            ));
        }
        Ok(found)
    }
}

/// Makes `arranged` in `document` (the retained one, edited) and in the one
/// file it writes, if any, then gives `definitions` (the running piece's)
/// each development's new layout, so `get_development` shows it without a
/// reload. The file is written last: on any refusal nothing changed.
pub(super) fn arrange(
    definitions: &mut DevelopmentDefinitions,
    document: &mut Invention,
    arranged: &[Arrangement],
) -> Result<(), RpcError> {
    {
        let loaded = definitions.loaded();
        let mut layouts = Vec::with_capacity(arranged.len());
        let mut file = None;
        for arrangement in arranged {
            let layout = match &arrangement.home {
                DevelopmentHome::Piece(chain) => {
                    let inline = crate::invention::reload::inline_definition_mut(document, chain);
                    // Every reload, timed or not, adopts the definitions of
                    // the document it retains, so this is found; were it
                    // ever absent, the loaded layout is what is shown.
                    let layout = arrangement.layout(inline.as_deref().map(|d| &d.layout));
                    if let Some(definition) = inline {
                        definition.layout = layout.clone();
                    }
                    layout
                }
                DevelopmentHome::File { path, within } => {
                    let layout = arrangement.layout(None);
                    file = Some((arrangement.index, path, within, layout.clone()));
                    layout
                }
                DevelopmentHome::Package => unreachable!("refused as read-only"),
            };
            layouts.push((arrangement.home.clone(), layout));
        }
        if let Some((index, path, within, layout)) = file {
            let root = DevelopmentHome::File {
                path: path.clone(),
                within: Vec::new(),
            };
            let own = loaded.at_home(&root).map(|entry| &entry.definition);
            let own = own.ok_or_else(|| RpcError::new(RpcErrorCode::Internal, "no file"))?;
            development_file::write_layout(path, within, own, &layout).map_err(
                |why| match why {
                    NotWritten::ReadOnly(message) => {
                        refused(index, EditFailureReason::ReadOnlyDevelopment, message)
                    }
                    NotWritten::Failed(message) => refused(
                        index,
                        EditFailureReason::DevelopmentNotWritten,
                        format!("{message}; nothing was applied"),
                    ),
                },
            )?;
        }
        if layouts.is_empty() {
            return Ok(());
        }
        let mut loaded: LoadedDevelopments = (**loaded).clone();
        for (home, layout) in &layouts {
            loaded.arrange(home, layout);
        }
        *definitions = DevelopmentDefinitions::of(Arc::new(loaded));
    }
    Ok(())
}
