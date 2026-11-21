//! Port of `foundation/kernel/keep.ts`: whether a component is being
//! destroyed.
//!
//! The original gives every instance a lifecycle a framework keeps and
//! releases, like React's `<Activity>` hiding it, and starts work outside the
//! instance through `whenKept`. A gpui entity is alive or dropped: work
//! outside it is a `Subscription` or a `Task` in one of its fields, which
//! ends with it, and what it does as it goes runs in `cx.on_release`. What
//! remains is `isDestroying`: work a teardown cuts short, like a drag, wasn't
//! ended by the person, so it commits nothing. A component runs its release
//! work inside [`while_destroying`], and `Controllable::commit` skips
//! commits there.

use std::cell::Cell;

thread_local! {
    static DESTROYING: Cell<usize> = const { Cell::new(0) };
}

/// Runs `run` as part of a component's teardown, like its `cx.on_release`
/// callback (`whileDestroying`).
pub fn while_destroying<R>(run: impl FnOnce() -> R) -> R {
    DESTROYING.with(|destroying| destroying.set(destroying.get() + 1));
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            DESTROYING.with(|destroying| destroying.set(destroying.get().saturating_sub(1)));
        }
    }
    let _done = Done;
    run()
}

/// Whether a component's teardown is running (`isDestroying`).
pub fn is_destroying() -> bool {
    DESTROYING.with(Cell::get) > 0
}
