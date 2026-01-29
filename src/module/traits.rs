/// The core abstraction for all synthesis components.
///
/// Every module in the synthesis graph implements this trait.
/// Modules process one sample at a time at audio rate.
pub trait Module: Send {
    /// Processes one sample of audio.
    ///
    /// Returns `true` if the module is still active, `false` if it should be removed.
    fn process(&mut self) -> bool {
        true
    }

    /// Returns the module's name for debugging purposes.
    fn name(&self) -> &str {
        "Module"
    }
}

/// A module that produces signals without requiring input.
///
/// Examples include oscillators, LFOs, clocks, and noise generators.
pub trait Generator<T>: Module {
    /// Generates and returns the next output sample.
    fn output(&mut self) -> T;
}

/// A module that transforms an input signal into an output signal.
///
/// Examples include filters, effects, and envelopes.
pub trait Processor<TIn, TOut>: Module {
    /// Processes an input signal and returns the transformed output.
    fn process_signal(&mut self, input: TIn) -> TOut;
}
