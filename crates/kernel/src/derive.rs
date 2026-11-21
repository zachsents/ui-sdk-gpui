//! Port of `foundation/kernel/derive.ts`: state that comes with other state,
//! what changes with options as they arrive, and what's worked out from the
//! rest of the state.
//!
//! The original registers derivations on its store so a framework that
//! renders options before it hands them over renders what they derive in
//! the same pass. A gpui component gets its options in `set_options`, and
//! nothing renders until it returns, so it derives there: the value
//! arriving, with the state before it, gives what changes with it, like the
//! panels a closing `value` leaves animating out. State worked out from the
//! rest is a method, current whenever it's read, or a [`Derived`] when
//! working it out costs enough to keep.

/// What arriving options derive (`deriveFromOptions`): `derive` gets the
/// value `sync` delivers and the one before it, when it's another value.
/// It's pure; a change made any other way, like a press, derives it where
/// it's made.
pub fn derive_from_options<T: PartialEq, R>(
    arriving: Option<&T>,
    before: &T,
    derive: impl FnOnce(&T, &T) -> R,
) -> Option<R> {
    match arriving {
        Some(arriving) if arriving != before => Some(derive(arriving, before)),
        _ => None,
    }
}

/// State worked out from other state, kept until what it's worked out from
/// changes (`deriveState`), like what each option of a list shows from the
/// query and the value.
#[derive(Clone, Debug)]
pub struct Derived<I, O> {
    inputs: Option<I>,
    output: Option<O>,
}

impl<I, O> Default for Derived<I, O> {
    fn default() -> Self {
        Self {
            inputs: None,
            output: None,
        }
    }
}

impl<I: PartialEq, O> Derived<I, O> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The state for `inputs`, worked out again only when they changed.
    pub fn get(&mut self, inputs: I, derive: impl FnOnce(&I) -> O) -> &O {
        if self.inputs.as_ref() != Some(&inputs) || self.output.is_none() {
            self.output = Some(derive(&inputs));
            self.inputs = Some(inputs);
        }
        self.output.as_ref().expect("derived just now")
    }

    /// The state as last worked out.
    pub fn last(&self) -> Option<&O> {
        self.output.as_ref()
    }

    /// Forgets it, so the next `get` works it out again.
    pub fn clear(&mut self) {
        self.inputs = None;
        self.output = None;
    }
}

/// `deriveState`: the state for `inputs` kept in `derived`.
pub fn derive_state<I: PartialEq, O>(
    derived: &mut Derived<I, O>,
    inputs: I,
    derive: impl FnOnce(&I) -> O,
) -> &O {
    derived.get(inputs, derive)
}
