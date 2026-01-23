use std::cell::RefCell;

/// Module trait - the core abstraction for all modular components
/// Like a Eurorack module that processes per-sample
pub trait Module: Send {
    /// Process one sample/step
    /// Returns true if the module is still active, false if it should be removed
    fn process(&mut self) -> bool {
        true
    }

    /// Get the module's name for debugging
    fn name(&self) -> &str {
        "Module"
    }
}

/// Generator trait - modules that produce signals without input
/// Examples: oscillators, LFOs, clocks, sequencers
pub trait Generator<T>: Module {
    fn output(&mut self) -> T;
}

/// Processor trait - modules that transform signals
/// Examples: filters, effects, envelopes
pub trait Processor<TIn, TOut>: Module {
    fn process_signal(&mut self, input: TIn) -> TOut;
}

/// Connect trait - allows modules to be connected with =>-style chaining
pub trait Connect<TOut>: Sized {
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

/// A connected chain of generator -> processor(s)
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

// Implement Connect for all Generators
impl<T, G> Connect<T> for G where G: Generator<T> {}
