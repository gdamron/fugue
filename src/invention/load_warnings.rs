//! What a load reports without refusing: a value it set aside, say, so the
//! piece still plays.
//!
//! A module's factory has no way back to the build that asked for it but
//! its result, so a warning goes to the build in force on this thread
//! instead: [`InventionBuilder`](super::InventionBuilder) collects each
//! module's warnings as it builds it, names the module, and keeps them on
//! the runtime it builds (see
//! [`RenderEngine::load_warnings`](super::RenderEngine::load_warnings)). A
//! build nested in a development's reaches its module's in turn, so a
//! warning names every instance it is inside. Builds run on control
//! threads; nothing here runs on the audio thread.

use std::cell::RefCell;
use std::fmt;

/// One thing a load set aside.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Warning {
    /// The modules it is in, outermost first.
    pub(crate) modules: Vec<String>,
    /// The control of the innermost development whose starting value it
    /// set aside, if that is what it is: a development passing its value
    /// down sets it aside too, and says so itself.
    pub(crate) refused: Option<String>,
    pub(crate) message: String,
}

#[cfg(test)]
impl Warning {
    /// A warning about the module being built.
    fn new(message: String) -> Self {
        Self {
            modules: Vec::new(),
            refused: None,
            message,
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for module in &self.modules {
            write!(f, "module '{module}': ")?;
        }
        f.write_str(&self.message)
    }
}

thread_local! {
    static COLLECTED: RefCell<Option<Vec<Warning>>> = const { RefCell::new(None) };
}

/// Reports `message` to the build in force, or to stderr when there is
/// none (a module built on its own, outside any document).
pub(crate) fn warn(message: Warning) {
    let unheard = COLLECTED.with(|collected| match collected.borrow_mut().as_mut() {
        Some(collected) => {
            collected.push(message);
            None
        }
        None => Some(message),
    });
    if let Some(message) = unheard {
        eprintln!("Warning: {message}");
    }
}

/// Runs `build`, returning what it reported apart from what it returns.
/// The build in force, if any, hears none of it.
pub(crate) fn collected<R>(build: impl FnOnce() -> R) -> (R, Vec<Warning>) {
    let previous = COLLECTED.with(|collected| collected.replace(Some(Vec::new())));
    let restore = Restore(Some(previous));
    let result = build();
    let warnings = restore.finish();
    (result, warnings)
}

/// Whether a build is collecting warnings on this thread.
pub(crate) fn collecting() -> bool {
    COLLECTED.with(|collected| collected.borrow().is_some())
}

/// Puts the collection in force before [`collected`] back, even by panic.
struct Restore(Option<Option<Vec<Warning>>>);

impl Restore {
    /// What was collected, with the previous collection back in force.
    fn finish(mut self) -> Vec<Warning> {
        let previous = self.0.take().flatten();
        let collected = COLLECTED.with(|collected| collected.replace(previous));
        collected.unwrap_or_default()
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            COLLECTED.with(|collected| *collected.borrow_mut() = previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_hears_what_it_runs_report() {
        let ((), warnings) = collected(|| warn(Warning::new("one".into())));
        assert_eq!(warnings, [Warning::new("one".into())]);
        assert!(!collecting());
    }

    #[test]
    fn a_warning_names_the_modules_it_is_in() {
        let mut warning = Warning::new("set aside".into());
        warning.modules = vec!["lead".into(), "nest".into()];
        assert_eq!(
            warning.to_string(),
            "module 'lead': module 'nest': set aside"
        );
    }

    #[test]
    fn a_nested_collection_keeps_its_warnings_from_the_outer() {
        let (inner, outer) = collected(|| {
            warn(Warning::new("outer".into()));
            let ((), inner) = collected(|| warn(Warning::new("inner".into())));
            assert!(collecting());
            inner
        });
        assert_eq!(inner, [Warning::new("inner".into())]);
        assert_eq!(outer, [Warning::new("outer".into())]);
    }
}
