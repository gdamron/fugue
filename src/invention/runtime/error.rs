/// Error type for graph command operations.
#[derive(Debug)]
pub enum GraphCommandError {
    /// The audio thread has stopped, so commands can no longer be delivered.
    AudioThreadStopped,
    /// The requested module type is not registered.
    UnknownModuleType(String),
    /// The module factory failed to build the module.
    ModuleBuildFailed(String),
    /// The referenced module does not exist in the graph.
    UnknownModule(String),
    /// The referenced port does not exist on the module.
    InvalidPort(String),
    /// A module control operation failed (invalid key, etc.).
    ControlError(String),
}

impl std::fmt::Display for GraphCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphCommandError::AudioThreadStopped => {
                write!(f, "audio thread has stopped; command not delivered")
            }
            GraphCommandError::UnknownModuleType(t) => {
                write!(f, "unknown module type: {}", t)
            }
            GraphCommandError::ModuleBuildFailed(msg) => {
                write!(f, "module build failed: {}", msg)
            }
            GraphCommandError::UnknownModule(id) => {
                write!(f, "unknown module: {}", id)
            }
            GraphCommandError::InvalidPort(msg) => {
                write!(f, "invalid port: {}", msg)
            }
            GraphCommandError::ControlError(msg) => {
                write!(f, "control error: {}", msg)
            }
        }
    }
}

impl std::error::Error for GraphCommandError {}
