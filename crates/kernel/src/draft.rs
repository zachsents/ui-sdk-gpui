//! Port of `foundation/kernel/draft.ts`: the end of a draft, a drag or
//! keyboard move that shows a value before it commits.

use gpui::{Context, Task};

/// A value a drag or keyboard move shows before it commits, and how it ends.
pub struct Draft<T> {
    value: Option<T>,
    // Each draft that starts, so a settle clears only the one it was for.
    generation: u64,
    settling: Option<Settling<T>>,
}

struct Settling<T> {
    before: T,
    generation: u64,
    _timer: Task<()>,
}

impl<T> Default for Draft<T> {
    fn default() -> Self {
        Self {
            value: None,
            generation: 0,
            settling: None,
        }
    }
}

impl<T: Clone + PartialEq + 'static> Draft<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The value shown while the draft lasts.
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub fn is_active(&self) -> bool {
        self.value.is_some()
    }

    /// Shows `value`, starting a draft or moving it.
    pub fn set(&mut self, value: T) {
        if self.value.is_none() || self.settling.is_some() {
            self.generation += 1;
            self.settling = None;
        }
        self.value = Some(value);
    }

    /// Ends the draft now.
    pub fn clear(&mut self) {
        self.value = None;
        self.settling = None;
    }

    /// Clears the draft once the value it committed arrives, so a controlled
    /// value never renders its old state between the drop and its new value
    /// (`settleDraft`). `value` is the value now and `before` the one from
    /// before the commit: an uncontrolled value has already moved on, so it
    /// clears now, and an app that kept the old value shows it again a tick
    /// later. Call `arrived` as the app passes values. `draft` finds this
    /// draft in the component.
    pub fn settle<C: 'static>(
        &mut self,
        value: &T,
        before: T,
        draft: fn(&mut C) -> &mut Draft<T>,
        cx: &mut Context<C>,
    ) {
        if *value != before {
            self.clear();
            return;
        }
        let generation = self.generation;
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::ZERO).await;
            this.update(cx, |component, cx| {
                let draft = draft(component);
                if draft.settling.as_ref().is_some_and(|settling| settling.generation == generation) {
                    draft.clear();
                    cx.notify();
                }
            })
            .ok();
        });
        self.settling = Some(Settling {
            before,
            generation,
            _timer: timer,
        });
    }

    /// A value arrived, like a controlled one the app passed. Clears a
    /// settling draft once it's another value than the one from before the
    /// commit. Returns whether it cleared.
    pub fn arrived(&mut self, value: &T) -> bool {
        let done = self
            .settling
            .as_ref()
            .is_some_and(|settling| settling.generation == self.generation && *value != settling.before);
        if done {
            self.clear();
        }
        done
    }
}
