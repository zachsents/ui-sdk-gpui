//! Port of `foundation/popups/sheet-effects.ts`: everything that follows a
//! sheet's position besides the sheet itself: the backdrop's dim, the
//! sheet's own progress, outlets like the window's content behind it, the
//! sheets it's stacked on, and the chrome color.
//!
//! The original writes these as CSS variables, hands each settle's curves to
//! WAAPI and ties them to a scroll timeline, so the main thread never writes
//! them frame by frame. gpui draws each frame itself: the effects are values
//! the sheet computes from its extent on every frame it moves, which its
//! parts style from ([`SheetValues`]), and which it hands its outlets and the
//! sheets beneath it, notifying the views that render them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{App, EntityId, SharedString};
use ui_sdk_motion::{dim_amount, expand_amount, progress_amount};
use ui_sdk_platform::Region;

/// What follows a sheet's position: the original's `--drawer-dim`,
/// `--drawer-progress` and `--drawer-expand`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SheetValues {
    /// How dark the backdrop is, from 0 to 1: 0 at or below the largest snap
    /// point the window stays usable at, 1 from the next one.
    pub dim: f64,
    /// 0 closed, 1 at the largest snap point. Never past 1.
    pub progress: f64,
    /// 0 at the second-largest snap point, 1 at the largest: where iOS pushes
    /// the presenting view back.
    pub expand: f64,
}

/// Keyframes for something that follows a sheet, by property, like
/// `("scale", [1.0, 0.94])` (`SheetKeyframes`). Index 0 is closed, the last
/// is fully expanded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetKeyframes(pub Vec<(SharedString, Vec<f32>)>);

impl SheetKeyframes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a property's keyframes.
    pub fn property(mut self, name: impl Into<SharedString>, frames: Vec<f32>) -> Self {
        self.0.push((name.into(), frames));
        self
    }

    /// A property's value at `amount` of the way from closed to expanded,
    /// between evenly spaced keyframes. At rest it's the first or the last.
    pub fn sample(&self, name: &str, amount: f64, rest: bool) -> Option<f32> {
        let frames = &self.0.iter().find(|(own, _)| own.as_ref() == name)?.1;
        match frames.len() {
            0 => None,
            1 => Some(frames[0]),
            count => {
                if rest {
                    return Some(frames[if amount >= 0.5 { count - 1 } else { 0 }]);
                }
                let position = amount.clamp(0.0, 1.0) * (count - 1) as f64;
                let index = (position.floor() as usize).min(count - 2);
                let fraction = (position - index as f64) as f32;
                Some(frames[index] + (frames[index + 1] - frames[index]) * fraction)
            }
        }
    }
}

struct OutletInner {
    view: Option<EntityId>,
    keyframes: Option<SheetKeyframes>,
    values: Cell<SheetValues>,
    rest: Cell<bool>,
}

/// Something that follows the sheet besides the backdrop (`SheetOutlet`): a
/// view, notified on every frame the sheet moves, that styles from
/// [`values`](Self::values) and its keyframes. Clones share it.
#[derive(Clone)]
pub struct SheetOutlet(Rc<OutletInner>);

impl std::fmt::Debug for SheetOutlet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SheetOutlet")
            .field("view", &self.0.view)
            .field("values", &self.0.values.get())
            .finish()
    }
}

impl SheetOutlet {
    /// An outlet `view` renders from, notified as the sheet moves.
    pub fn new(view: Option<EntityId>) -> Self {
        Self(Rc::new(OutletInner {
            view,
            keyframes: None,
            values: Cell::new(SheetValues::default()),
            rest: Cell::new(true),
        }))
    }

    /// An outlet that plays `keyframes` as the sheet expands.
    pub fn with_keyframes(view: Option<EntityId>, keyframes: SheetKeyframes) -> Self {
        Self(Rc::new(OutletInner {
            view,
            keyframes: Some(keyframes),
            values: Cell::new(SheetValues::default()),
            rest: Cell::new(true),
        }))
    }

    /// The sheet's values as of its last frame.
    pub fn values(&self) -> SheetValues {
        self.0.values.get()
    }

    /// A keyframed property's value now.
    pub fn keyframe(&self, name: &str) -> Option<f32> {
        let keyframes = self.0.keyframes.as_ref()?;
        keyframes.sample(name, self.0.values.get().expand, self.0.rest.get())
    }

    fn write(&self, values: SheetValues, rest: bool, cx: &mut App) {
        self.0.values.set(values);
        self.0.rest.set(rest);
        if let Some(view) = self.0.view {
            cx.notify(view);
        }
    }
}

struct StackInner {
    region: Region,
    stack: Cell<f64>,
    owner: Option<EntityId>,
    parent: RefCell<Option<SheetStack>>,
}

/// A sheet as the sheets stacked on it see it: its element, how far the
/// sheets above push it back (`--drawer-stack`), and the sheet it's stacked
/// on in turn. Clones share it.
#[derive(Clone)]
pub struct SheetStack(Rc<StackInner>);

impl PartialEq for SheetStack {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for SheetStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SheetStack")
            .field("stack", &self.0.stack.get())
            .finish()
    }
}

impl SheetStack {
    pub(crate) fn new(region: Region, owner: Option<EntityId>) -> Self {
        Self(Rc::new(StackInner {
            region,
            stack: Cell::new(0.0),
            owner,
            parent: RefCell::new(None),
        }))
    }

    /// The sheet's element.
    pub fn element(&self) -> Region {
        self.0.region.clone()
    }

    /// How far the sheets above push it back: its depth below them, plus how
    /// far the nearest has opened (`--drawer-stack`).
    pub fn stack(&self) -> f64 {
        self.0.stack.get()
    }

    /// The sheet this one is stacked on.
    pub fn parent(&self) -> Option<SheetStack> {
        self.0.parent.borrow().clone()
    }

    pub(crate) fn set_parent(&self, parent: Option<SheetStack>) {
        *self.0.parent.borrow_mut() = parent;
    }

    fn write(&self, stack: f64, cx: &mut App) {
        if self.0.stack.replace(stack) != stack
            && let Some(owner) = self.0.owner
        {
            cx.notify(owner);
        }
    }
}

/// What a sheet's effects reach (`createSheetEffects`'s targets).
pub struct SheetEffectsTargets {
    pub outlets: Rc<dyn Fn() -> Vec<SheetOutlet>>,
    /// The sheets this one is stacked on, nearest first.
    pub ancestors: Rc<dyn Fn() -> Vec<SheetStack>>,
    /// Writes the chrome color for a dim amount, or nothing when off.
    pub theme_color: Option<Rc<dyn Fn(f64, &mut App)>>,
}

struct EffectsState {
    snap_extents: Vec<f64>,
    undimmed_index: Option<usize>,
    values: SheetValues,
    destroyed: bool,
}

/// A sheet's effects. Clones share them.
#[derive(Clone)]
pub struct SheetEffects {
    state: Rc<RefCell<EffectsState>>,
    targets: Rc<SheetEffectsTargets>,
}

fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// A sheet's effects over what they reach (`createSheetEffects`).
pub fn create_sheet_effects(targets: SheetEffectsTargets) -> SheetEffects {
    SheetEffects {
        state: Rc::new(RefCell::new(EffectsState {
            snap_extents: vec![1.0],
            undimmed_index: None,
            values: SheetValues::default(),
            destroyed: false,
        })),
        targets: Rc::new(targets),
    }
}

impl SheetEffects {
    /// The snap points' extents, ascending, and the largest the window stays
    /// usable and undimmed at.
    pub fn configure(&self, snap_extents: Vec<f64>, undimmed_index: Option<usize>) {
        let mut state = self.state.borrow_mut();
        state.snap_extents = if snap_extents.is_empty() { vec![1.0] } else { snap_extents };
        state.undimmed_index = undimmed_index;
    }

    /// The values at `extent`.
    pub fn compute(&self, extent: f64) -> SheetValues {
        let state = self.state.borrow();
        SheetValues {
            dim: round(dim_amount(extent, &state.snap_extents, state.undimmed_index)),
            progress: round(progress_amount(extent, &state.snap_extents)),
            expand: round(expand_amount(extent, &state.snap_extents)),
        }
    }

    /// The values as of the last frame.
    pub fn values(&self) -> SheetValues {
        self.state.borrow().values
    }

    /// Follows the sheet at `extent`, for a frame it moved in, or at rest,
    /// when the chrome color follows too.
    pub fn apply(&self, extent: f64, rest: bool, cx: &mut App) {
        if self.state.borrow().destroyed {
            return;
        }
        let values = self.compute(extent);
        self.state.borrow_mut().values = values;
        for (depth, ancestor) in (self.targets.ancestors)().iter().enumerate() {
            ancestor.write(depth as f64 + values.progress, cx);
        }
        for outlet in (self.targets.outlets)() {
            outlet.write(values, rest, cx);
        }
        if rest && let Some(theme_color) = &self.targets.theme_color {
            theme_color(values.dim, cx);
        }
    }

    /// Puts what follows the sheet back as it was before it opened.
    pub fn destroy(&self, cx: &mut App) {
        {
            let mut state = self.state.borrow_mut();
            state.destroyed = true;
            state.values = SheetValues::default();
        }
        for outlet in (self.targets.outlets)() {
            outlet.write(SheetValues::default(), true, cx);
        }
    }
}
