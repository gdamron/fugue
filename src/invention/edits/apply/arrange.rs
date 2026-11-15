//! `move_modules` inside a development: the arrangement saved in the
//! development's own document, the piece's (for an inline definition) or
//! its file, and never in an installed package's: inside a development the
//! piece borrows, the move is saved in the piece's layout as an override
//! (see [`crate::invention::layout_overrides`]). Presentation only, as a
//! move of the piece's own modules: nothing is built, published or heard,
//! and it all runs on the calling control thread.

use std::path::Path;
use std::sync::Arc;

use crate::invention::development_file::{self, NotWritten, Staged};
use crate::invention::layout::{Layout, Position};
use crate::invention::layout_overrides;
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
    /// The path that names it, which keys its overrides.
    path: Vec<String>,
    home: DevelopmentHome,
    /// Whether its moves are overrides in the piece's layout: the piece
    /// borrows it.
    overrides: bool,
    base: Layout,
    moves: Vec<(String, Option<Position>)>,
}

impl Arrangement {
    /// This arrangement, with its file and the chain inside it, when it is
    /// saved in a development file.
    fn in_file(&self) -> Option<(&Self, &std::path::PathBuf, &[String])> {
        match &self.home {
            DevelopmentHome::File { path, within } if !self.overrides => Some((self, path, within)),
            _ => None,
        }
    }

    /// Whether `other`'s moves belong with this one's: the same document,
    /// or for overrides the same path.
    fn joins(&self, home: &DevelopmentHome, path: &[String], overrides: bool) -> bool {
        match (self.overrides, overrides) {
            (true, true) => self.path == path,
            (false, false) => &self.home == home,
            _ => false,
        }
    }

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

/// Whether the development at `home` is borrowed: it is never written here
/// (an installed package's, a file in the package cache, or one a frozen
/// lockfile locks), so a piece arranges it with overrides.
pub(crate) fn borrowed(home: &DevelopmentHome) -> bool {
    match home {
        DevelopmentHome::Piece(_) => false,
        DevelopmentHome::File { path, .. } => development_file::never_written(path).is_some(),
        DevelopmentHome::Package => true,
    }
}

/// Where a move inside the development at `home` is saved, in a piece read
/// from `piece` (its source path, if any). A piece that is itself an
/// installed package's file (an installed example) is never saved here, so
/// nothing inside it is arranged.
pub(crate) fn saved_in(home: &DevelopmentHome, piece: Option<&Path>) -> DevelopmentSavedIn {
    if piece.and_then(development_file::never_written).is_some() {
        return DevelopmentSavedIn::ReadOnly {
            reason: "the piece comes from an installed package and is never saved here; \
                     save it as your own piece to arrange it"
                .to_string(),
        };
    }
    match home {
        DevelopmentHome::Piece(_) => DevelopmentSavedIn::Piece,
        _ if borrowed(home) => DevelopmentSavedIn::Override,
        DevelopmentHome::File { path, .. } => DevelopmentSavedIn::File {
            path: path.display().to_string(),
        },
        DevelopmentHome::Package => unreachable!("a package's is borrowed"),
    }
}

impl RunningInvention {
    /// The development moves in `edits`, one arrangement per development,
    /// each checked: the development exists and is saved somewhere it may
    /// be written, and each id is one of its own modules. At most one file
    /// is written per batch, and its new text is staged here, before any
    /// lock is taken (see [`development_file::stage_layout`]).
    pub(super) fn arrangements(&self, edits: &[StructuralEdit]) -> Result<Arranged, RpcError> {
        let (definitions, piece) = self.definitions_and_source();
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
            let saved = saved_in(&entry.home, piece.as_deref());
            if let DevelopmentSavedIn::ReadOnly { reason } = saved {
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
            let overrides = saved == DevelopmentSavedIn::Override;
            if overrides {
                if let Some(id) =
                    (positions.keys()).find(|id| layout_overrides::key(path, id).is_none())
                {
                    return Err(refused(
                        index,
                        EditFailureReason::ReadOnlyDevelopment,
                        format!(
                            "module '{id}' inside development '{named}' cannot be overridden: \
                             an override is keyed by names without '/'"
                        ),
                    ));
                }
            }
            let moves = positions.iter().map(|(id, at)| (id.clone(), *at));
            match (found.iter_mut()).find(|known| known.joins(&entry.home, path, overrides)) {
                Some(known) => known.moves.extend(moves),
                None => found.push(Arrangement {
                    index,
                    path: path.to_vec(),
                    home: entry.home.clone(),
                    overrides,
                    base: entry.definition.layout.clone(),
                    moves: moves.collect(),
                }),
            }
        }
        let files = found.iter().filter_map(Arrangement::in_file);
        if files.count() > 1 {
            return Err(RpcError::new(
                RpcErrorCode::InvalidRequest,
                "one apply_edits batch arranges at most one development saved in a \
                 file; nothing was applied",
            ));
        }
        let staged = match found.iter().find_map(Arrangement::in_file) {
            Some((arrangement, path, within)) => {
                let root = DevelopmentHome::File {
                    path: path.clone(),
                    within: Vec::new(),
                };
                let own = loaded.at_home(&root).map(|entry| &entry.definition);
                let own = own.ok_or_else(|| RpcError::new(RpcErrorCode::Internal, "no file"))?;
                let layout = arrangement.layout(None);
                let staged = development_file::stage_layout(path, within, own, &layout)
                    .map_err(|why| not_written(arrangement.index, why))?;
                Some((arrangement.index, staged))
            }
            None => None,
        };
        Ok(Arranged {
            definitions,
            found,
            staged,
        })
    }
}

/// A batch's arrangements, with the definitions they were checked against
/// and the one file's new text, staged.
pub(super) struct Arranged {
    definitions: DevelopmentDefinitions,
    found: Vec<Arrangement>,
    staged: Option<(usize, Staged)>,
}

impl Arranged {
    /// Whether `definitions` (the running piece's, under the lock) are the
    /// ones these arrangements were checked against: not, when a timed
    /// reload landed as the lock was taken.
    pub(super) fn checked_against(&self, definitions: &DevelopmentDefinitions) -> bool {
        self.found.is_empty() || Arc::ptr_eq(definitions.loaded(), self.definitions.loaded())
    }
}

/// The refusal of the file write a development move at `index` needs.
fn not_written(index: usize, why: NotWritten) -> RpcError {
    match why {
        NotWritten::ReadOnly(message) => {
            refused(index, EditFailureReason::ReadOnlyDevelopment, message)
        }
        NotWritten::Failed(message) => {
            refused(index, EditFailureReason::DevelopmentNotWritten, message)
        }
    }
}

/// Makes `arranged` in `document` (the retained one, edited) and in the one
/// file it writes, if any, then gives `definitions` (the running piece's)
/// each development's new layout, so `get_development` shows it without a
/// reload. The caller has checked that `definitions` are still those the
/// moves were checked against ([`Arranged::checked_against`]). The file is
/// renamed into place last: on any refusal nothing changed, and the staged
/// file is removed.
pub(super) fn arrange(
    definitions: &mut DevelopmentDefinitions,
    document: &mut Invention,
    arranged: Arranged,
) -> Result<(), RpcError> {
    let Arranged { found, staged, .. } = arranged;
    if found.is_empty() {
        return Ok(());
    }
    let mut layouts = Vec::with_capacity(found.len());
    for arrangement in &found {
        if arrangement.overrides {
            override_moves(document, arrangement);
            continue;
        }
        let layout = match &arrangement.home {
            DevelopmentHome::Piece(chain) => {
                let inline = crate::invention::reload::inline_definition_mut(document, chain);
                // The definitions are the retained document's own (checked
                // above), so this is found; were it ever absent, the loaded
                // layout is what is shown.
                let layout = arrangement.layout(inline.as_deref().map(|d| &d.layout));
                if let Some(definition) = inline {
                    definition.layout = layout.clone();
                }
                layout
            }
            DevelopmentHome::File { .. } => arrangement.layout(None),
            DevelopmentHome::Package => unreachable!("a package's is arranged with overrides"),
        };
        layouts.push((arrangement.home.clone(), layout));
    }
    if let Some((index, staged)) = staged {
        staged.place().map_err(|why| not_written(index, why))?;
    }
    if layouts.is_empty() {
        return Ok(());
    }
    let mut loaded: LoadedDevelopments = (**definitions.loaded()).clone();
    for (home, layout) in &layouts {
        loaded.arrange(home, layout);
    }
    *definitions = definitions.with_loaded(Arc::new(loaded));
    Ok(())
}

/// Makes `arrangement`'s moves as overrides in the piece's `document`: a
/// position overrides the author's, and `null` removes the override, back
/// to where the author put the module. The definition is left as loaded.
fn override_moves(document: &mut Invention, arrangement: &Arrangement) {
    for (id, position) in &arrangement.moves {
        let key = layout_overrides::key(&arrangement.path, id).expect("checked above");
        match position {
            Some(position) => document.layout.overrides.insert(key, *position),
            None => document.layout.overrides.remove(&key),
        };
    }
}
