//! Port of `foundation/popups/dismissal.ts`: `dismissible` and
//! `on_dismiss_attempt`, shared by every layer a person can dismiss: whether
//! Escape, an outside press, focus leaving, back and a swipe may close it,
//! and what the app hears when one is refused.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, Context, Window};
use ui_sdk_kernel::{DismissAttemptDetails, DismissAttemptReason, InputEvent, Reason, after_update, is_dismissal};

/// Hears a refused dismissal made on purpose. It runs once the component's
/// update is over, so it may update any entity, the component too.
pub type DismissAttemptCallback = Rc<dyn Fn(&DismissAttemptDetails, &mut Window, &mut App)>;

/// The options a dismissable layer takes (`DismissOptions`).
#[derive(Clone)]
pub struct DismissOptions {
    /// Whether Escape, an outside press, focus or the window leaving, the
    /// back gesture and a sheet's swipe can close it. Triggers, Close parts
    /// and the controller always can. Pass `false` while it holds unsaved
    /// work, and ask in `on_dismiss_attempt`. Defaults to true.
    pub dismissible: bool,
    /// Runs when `dismissible` refused a dismissal made on purpose: Escape,
    /// an outside press, back or a swipe. Focus or the window leaving is
    /// refused without it. Ask "Discard changes?" here.
    pub on_dismiss_attempt: Option<DismissAttemptCallback>,
}

impl Default for DismissOptions {
    fn default() -> Self {
        Self {
            dismissible: true,
            on_dismiss_attempt: None,
        }
    }
}

impl DismissOptions {
    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn on_dismiss_attempt(
        mut self,
        callback: impl Fn(&DismissAttemptDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss_attempt = Some(Rc::new(callback));
        self
    }
}

/// How a close is refused: `Attempt` refuses it and tells
/// `on_dismiss_attempt` when someone tried on purpose, and `Silent` refuses
/// it without a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refusal {
    Attempt,
    Silent,
}

/// How a layer refuses a close, or `None` to leave it to `dismissible`.
pub type RefusalFn<T> = Rc<dyn Fn(&T, Reason) -> Option<Refusal>>;

/// Runs after `on_dismiss_attempt` heard a refused attempt, like a sheet
/// nudging, inside the component's update.
pub type RefusedFn<T> = Rc<dyn Fn(&mut T, DismissAttemptReason, &mut Window, &mut Context<T>)>;

/// What a layer refuses beyond `dismissible`, like Dialog's closes while an
/// action runs (`DismissRules`).
pub struct DismissRules<T> {
    pub refusal: Option<RefusalFn<T>>,
    pub refused: Option<RefusedFn<T>>,
}

impl<T> Default for DismissRules<T> {
    fn default() -> Self {
        Self {
            refusal: None,
            refused: None,
        }
    }
}

impl<T> Clone for DismissRules<T> {
    fn clone(&self) -> Self {
        Self {
            refusal: self.refusal.clone(),
            refused: self.refused.clone(),
        }
    }
}

/// What a sheet presentation takes from a dismissal policy: whether a swipe
/// can dismiss it, and the attempt a refused swipe makes.
#[derive(Clone)]
pub struct SheetDismissal {
    pub dismissible: Rc<dyn Fn() -> bool>,
    pub on_dismiss_attempt: Rc<dyn Fn(Option<&InputEvent>, &mut Window, &mut App)>,
}

/// The dismissal policy of a layer, from its options and any rules of its
/// own (`createDismissal`). [`Dismissal::allows`] decides a close, and
/// [`Dismissal::sheet`] is what its sheet presentation takes. Clones share it.
pub struct Dismissal<T> {
    options: Rc<RefCell<DismissOptions>>,
    rules: Rc<RefCell<DismissRules<T>>>,
}

impl<T> Clone for Dismissal<T> {
    fn clone(&self) -> Self {
        Self {
            options: self.options.clone(),
            rules: self.rules.clone(),
        }
    }
}

/// A layer's dismissal policy (`createDismissal`).
pub fn create_dismissal<T: 'static>(options: DismissOptions, rules: DismissRules<T>) -> Dismissal<T> {
    Dismissal {
        options: Rc::new(RefCell::new(options)),
        rules: Rc::new(RefCell::new(rules)),
    }
}

fn attempt(options: &Rc<RefCell<DismissOptions>>, reason: DismissAttemptReason, event: Option<&InputEvent>, window: &mut Window, cx: &mut App) {
    let callback = options.borrow().on_dismiss_attempt.clone();
    if let Some(callback) = callback {
        let details = DismissAttemptDetails {
            reason,
            event: event.cloned(),
        };
        after_update(window, cx, move |window, cx| callback(&details, window, cx));
    }
}

impl<T: 'static> Dismissal<T> {
    /// New options, as the component's arrive.
    pub fn set_options(&self, options: DismissOptions) {
        *self.options.borrow_mut() = options;
    }

    /// The layer's own refusals.
    pub fn set_rules(&self, rules: DismissRules<T>) {
        *self.rules.borrow_mut() = rules;
    }

    /// Whether `dismissible` lets a person dismiss it now.
    pub fn dismissible(&self) -> bool {
        self.options.borrow().dismissible
    }

    /// Decides a close: false refuses it, after telling `on_dismiss_attempt`
    /// about a refused attempt made on purpose.
    pub fn allows(
        &self,
        this: &mut T,
        reason: Reason,
        event: Option<&InputEvent>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        let (refusal, refused) = {
            let rules = self.rules.borrow();
            (rules.refusal.clone(), rules.refused.clone())
        };
        let refused_how = refusal
            .and_then(|refusal| refusal(this, reason))
            .or_else(|| (is_dismissal(reason) && !self.dismissible()).then_some(Refusal::Attempt));
        let Some(how) = refused_how else {
            return true;
        };
        // Focus or the window leaving isn't a try to dismiss it.
        if how == Refusal::Attempt
            && reason != Reason::FocusOut
            && reason != Reason::WindowBlur
            && let Some(attempted) = DismissAttemptReason::of(reason)
        {
            attempt(&self.options, attempted, event, window, cx);
            if let Some(refused) = refused {
                refused(this, attempted, window, cx);
            }
        }
        false
    }

    /// What its sheet presentation takes: whether a swipe can dismiss it, and
    /// the attempt a refused swipe makes.
    pub fn sheet(&self) -> SheetDismissal {
        let options = self.options.clone();
        let attempts = self.options.clone();
        SheetDismissal {
            dismissible: Rc::new(move || options.borrow().dismissible),
            on_dismiss_attempt: Rc::new(move |event, window, cx| {
                attempt(&attempts, DismissAttemptReason::Swipe, event, window, cx)
            }),
        }
    }
}
