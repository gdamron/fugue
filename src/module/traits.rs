use std::cell::RefCell;

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

/// Enables chaining modules together using the `connect` method.
///
/// This trait is automatically implemented for all [`Generator`] types.
pub trait Connect<TOut>: Sized {
    /// Connects this generator to a processor, creating a signal chain.
    fn connect<TIn, P>(self, processor: P) -> ConnectedProcessor<Self, P, TOut, TIn>
    where
        P: Processor<TOut, TIn>,
        TOut: Clone,
    {
        ConnectedProcessor {
            source: RefCell::new(self),
            processor: RefCell::new(processor),
            _phantom: std::marker::PhantomData,
        }
    }
}

/// A chain of a generator connected to a processor.
///
/// This struct is created by [`Connect::connect`] and itself implements
/// [`Generator`], allowing chains to be extended further.
pub struct ConnectedProcessor<G, P, TOut, TIn> {
    source: RefCell<G>,
    processor: RefCell<P>,
    _phantom: std::marker::PhantomData<(TOut, TIn)>,
}

impl<G, P, TOut, TIn> ConnectedProcessor<G, P, TOut, TIn>
where
    G: Generator<TOut>,
    P: Processor<TOut, TIn>,
    TOut: Clone + Send,
    TIn: Send,
{
    /// Generates output by pulling from the source and processing through the chain.
    pub fn output(&mut self) -> TIn {
        let signal = self.source.borrow_mut().output();
        self.processor.borrow_mut().process_signal(signal)
    }
}

impl<G, P, TOut, TIn> Module for ConnectedProcessor<G, P, TOut, TIn>
where
    G: Generator<TOut>,
    P: Processor<TOut, TIn>,
    TOut: Clone + Send,
    TIn: Send,
{
    fn process(&mut self) -> bool {
        self.source.borrow_mut().process() && self.processor.borrow_mut().process()
    }
}

impl<G, P, TOut, TIn> Generator<TIn> for ConnectedProcessor<G, P, TOut, TIn>
where
    G: Generator<TOut>,
    P: Processor<TOut, TIn>,
    TOut: Clone + Send,
    TIn: Send,
{
    fn output(&mut self) -> TIn {
        let signal = self.source.borrow_mut().output();
        self.processor.borrow_mut().process_signal(signal)
    }
}

impl<T, G> Connect<T> for G where G: Generator<T> {}
