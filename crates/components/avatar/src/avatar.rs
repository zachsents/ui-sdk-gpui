//! Port of `components/avatar/avatar.ts`.
//!
//! Avatar shows a person's image, and a fallback, like their initials, until
//! the image loads or when it can't. The image is gpui's `img` from the first
//! frame its part draws, loading once through `img`'s own caches, and it
//! stays out of sight until it has loaded. One already in the cache is known
//! to be loaded as its part lays out, before the frame paints, so the
//! fallback never flashes for it: the parts show what the image found that
//! frame, and its change reports it right after.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyImageCache, App, Context, Div, Entity, ImageCache, ImageCacheError, ImageId, ImageSource,
    ImageStyle, Img, InteractiveElement, Interactivity, IntoElement, ParentElement, Refineable,
    RenderImage, Resource, SharedString, Stateful, StatefulInteractiveElement, StyleRefinement,
    Styled, StyledImage, Subscription, Task, WeakEntity, Window, img, px,
};

use ui_sdk_image::use_image;
use ui_sdk_kernel::bound_keys::bound;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentState, Aria, ChangeCallback, ChangeDetails, ChangeOptions, Controllable, Instance,
    Part, PartElement, Reason, held,
};

use crate::image_status::image_status;

/// Where the image is: no image, loading, loaded, or failed.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AvatarLoadingStatus {
    /// No image is in the page.
    #[default]
    Idle,
    Loading,
    Loaded,
    /// It failed, or it has no source.
    Error,
}

impl AvatarLoadingStatus {
    /// The original's name for it, like `"loaded"` (`data-loading-status`).
    pub fn as_str(self) -> &'static str {
        match self {
            AvatarLoadingStatus::Idle => "idle",
            AvatarLoadingStatus::Loading => "loading",
            AvatarLoadingStatus::Loaded => "loaded",
            AvatarLoadingStatus::Error => "error",
        }
    }
}

// The original's name, which `debug` logs and the watchers read.
impl fmt::Debug for AvatarLoadingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The options `Avatar` takes.
#[derive(Clone, Default)]
pub struct AvatarOptions {
    /// Runs when the image's loading status changes: `Loading` when it gets a
    /// source, `Loaded`, `Error`, and `Idle` when it leaves the page.
    /// `details.reason` says why: `ImageLoad`, `ImageError` or `SrcChange`.
    /// `details.event` is `None`, since gpui's `img` has no load or error
    /// event. It runs inside the avatar's update, so it may update any entity
    /// but the avatar.
    pub on_loading_status_change: Option<ChangeCallback<AvatarLoadingStatus>>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl AvatarOptions {
    pub fn on_loading_status_change(
        mut self,
        callback: impl Fn(&AvatarLoadingStatus, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_loading_status_change = Some(Rc::new(callback));
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `fallback` slot of `Avatar` takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AvatarFallbackProps {
    /// How long to wait before showing the fallback, so a fast image appears
    /// without the fallback flashing first. Without one, the fallback shows
    /// at once. Zero by default.
    pub delay: Duration,
}

impl AvatarFallbackProps {
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// What `with` styling and the app read. Every part gets the same state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvatarState {
    /// Where the image is: `Idle`, `Loading`, `Loaded` or `Error`.
    pub loading_status: AvatarLoadingStatus,
}

// Until it loads, the image is in the page but out of sight, out of the
// accessibility tree and out of the layout: a 1px box out of the flow, which
// `invisible` keeps from painting or taking pointer events, the original's
// `opacity: 0` and `pointer-events: none`.
fn hidden_image_style() -> StyleRefinement {
    StyleRefinement::default()
        .absolute()
        .w(px(1.))
        .h(px(1.))
        .invisible()
}

// An image with no source can never load. An empty path or URL is none, as
// an empty `src` is.
fn has_source(source: &ImageSource) -> bool {
    match source {
        ImageSource::Resource(Resource::Uri(uri)) => !uri.is_empty(),
        ImageSource::Resource(Resource::Path(path)) => !path.as_os_str().is_empty(),
        ImageSource::Resource(Resource::Embedded(path)) => !path.is_empty(),
        ImageSource::Render(_) | ImageSource::Image(_) | ImageSource::Custom(_) => true,
    }
}

// What tells one source from another, where the original watches the
// `src`, `srcset`, `sizes` and `crossorigin` attributes: a new one starts
// over.
#[derive(Clone, Debug, PartialEq)]
enum SourceKey {
    Resource(Resource),
    Render(ImageId),
    Image(u64),
    // A custom loader, by its address: a new closure is a new source.
    Custom(usize),
}

impl SourceKey {
    fn of(source: &ImageSource) -> Self {
        match source {
            ImageSource::Resource(resource) => SourceKey::Resource(resource.clone()),
            ImageSource::Render(image) => SourceKey::Render(image.id),
            ImageSource::Image(image) => SourceKey::Image(image.id()),
            ImageSource::Custom(load) => SourceKey::Custom(Arc::as_ptr(load) as *const () as usize),
        }
    }
}

// What the avatar's parts read as they draw, shared with them.
#[derive(Default)]
struct Live {
    // The status the parts show: the one that stands, or the one the image
    // found this frame, which its change reports right after.
    status: Cell<AvatarLoadingStatus>,
    // The `delay` the fallback rendered with last.
    fallback_delay: Cell<Duration>,
    image: RefCell<Watched>,
    // What the image rendered this frame watches, alive while the frame that
    // built it draws: an image that leaves the page leaves the fallback in
    // its place that frame.
    watch: RefCell<Weak<ImageWatch>>,
}

// What the image found as it drew.
#[derive(Default)]
struct Watched {
    // The source it drew with last, and its status then: `None` until it
    // draws, and again once it leaves the page.
    seen: Option<(SourceKey, AvatarLoadingStatus)>,
    // The last image that loaded.
    loaded: Option<Arc<RenderImage>>,
    // What `img` draws.
    drawn: Option<Result<Arc<RenderImage>, ImageCacheError>>,
}

/// An avatar. Its entity is its controller.
pub struct Avatar {
    instance: Instance,
    loading_status: Controllable<AvatarLoadingStatus>,
    // Whether the longest fallback delay has passed since the avatar bound.
    delay_elapsed: bool,
    live: Rc<Live>,
    // Whether the root is on the page, which starts the delay.
    root_bound: bool,
    delay: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Avatar {
    pub fn new(options: AvatarOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut loading_status = Controllable::new("loadingStatus", AvatarLoadingStatus::Idle);
        loading_status.set_on_change(options.on_loading_status_change);
        let mut instance = Instance::new("avatar").with_agent(cx);
        instance.debug = options.debug;
        // Asking whether the root and the image are in the page notifies the
        // avatar as they come and go, where the original binds them.
        bound(&instance, "root");
        bound(&instance, "image");
        let this = cx.entity();
        let subscriptions = vec![
            cx.observe_in(&this, window, |avatar, _, window, cx| {
                avatar.bound_changed(window, cx)
            }),
            cx.on_release_in(window, |avatar, window, cx| avatar.released(window, cx)),
        ];
        Self {
            instance,
            loading_status,
            delay_elapsed: false,
            live: Rc::default(),
            root_bound: false,
            delay: None,
            _subscriptions: subscriptions,
        }
    }

    /// Hands it new options, as a framework re-renders with new props (`sync`).
    pub fn set_options(&mut self, options: AvatarOptions, _cx: &mut Context<Self>) {
        self.loading_status
            .set_on_change(options.on_loading_status_change);
        self.instance.debug = options.debug;
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn state(&self) -> AvatarState {
        AvatarState {
            loading_status: *self.loading_status.get(),
        }
    }

    /// Where the image is.
    pub fn loading_status(&self) -> AvatarLoadingStatus {
        *self.loading_status.get()
    }

    // A load or an error is never the user's doing, so no change reads
    // input: its source comes from its reason alone.
    fn report(
        &mut self,
        status: AvatarLoadingStatus,
        reason: Reason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = self.loading_status.change(
            &self.instance,
            status,
            ChangeOptions::reason(reason),
            window,
            cx,
        );
        // A canceled change takes back what the parts showed.
        let stands = *self.loading_status.get();
        let shown = self.live.status.replace(stands);
        if changed || shown != stands {
            cx.notify();
        }
    }

    // The root or the image came or went. The root's arrival starts the
    // fallback's delay and its leaving stops it, and an image that left the
    // page takes its status with it.
    fn bound_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = bound(&self.instance, "root");
        if root != self.root_bound {
            self.root_bound = root;
            if root {
                self.start_delay(cx);
            } else {
                self.delay = None;
            }
        }
        if !bound(&self.instance, "image") {
            *self.live.image.borrow_mut() = Watched::default();
            if *self.loading_status.get() != AvatarLoadingStatus::Idle {
                self.report(AvatarLoadingStatus::Idle, Reason::SrcChange, window, cx);
            }
        }
    }

    // The avatar went while its image was in the page, which no frame will
    // unbind now, so the image's status goes with it, as the original's
    // image cleanup reports after the avatar unmounts.
    fn released(&mut self, window: &mut Window, cx: &mut App) {
        if *self.loading_status.get() != AvatarLoadingStatus::Idle
            && held(&self.instance, "image", None)
        {
            self.loading_status.change(
                &self.instance,
                AvatarLoadingStatus::Idle,
                ChangeOptions::reason(Reason::SrcChange),
                window,
                cx,
            );
        }
    }

    // Starts the fallback's delay once the avatar is on the page. The
    // fallback rendered in the frame that put it there, so its `delay` is
    // the newest.
    fn start_delay(&mut self, cx: &mut Context<Self>) {
        let delay = self.live.fallback_delay.get();
        if delay.is_zero() || self.delay_elapsed {
            return;
        }
        self.delay = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |avatar, cx| {
                avatar.delay_elapsed = true;
                cx.notify();
            })
            .ok();
        }));
    }

    /// The `root` part: a box for the image and its fallback. Set its size,
    /// and let the image and the fallback fill it.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<AvatarState> {
        let avatar = this.read(cx);
        Part::new(&avatar.instance, "root", None, avatar.state())
    }

    /// The `image` part: gpui's `img` for `source`, out of sight and out of
    /// the accessibility tree until it loads. Its `aria_label`, the
    /// original's `alt`, names the avatar then. The image fills the part, so
    /// size the part, and round it to round the image, since gpui clips
    /// nothing to rounded corners. An empty path or URL is no source, which
    /// fails at once.
    pub fn image(this: &Entity<Self>, source: impl Into<ImageSource>, cx: &App) -> AvatarImage {
        let avatar = this.read(cx);
        let drawn = avatar.live.clone();
        AvatarImage {
            part: Part::new(&avatar.instance, "image", None, avatar.state()).aria_role(Role::Img),
            img: img(move |_: &mut Window, _: &mut App| drawn.image.borrow().drawn.clone())
                .id("image")
                .size_full(),
            source: source.into(),
            cache: None,
            avatar: this.downgrade(),
            live: avatar.live.clone(),
        }
    }

    /// The `fallback` part: shown while there's no loaded image, after its
    /// `delay`, which it decides as it lays out. A screen reader reads text
    /// in it only when it's gpui's `text!`, not a plain string. To show
    /// initials and have it hear the person's name, as visually hidden text
    /// does, give the part a role, like `Role::Img`, and an `aria_label`.
    pub fn fallback(this: &Entity<Self>, props: AvatarFallbackProps, cx: &App) -> Part<AvatarState> {
        let avatar = this.read(cx);
        avatar.live.fallback_delay.set(props.delay);
        let live = avatar.live.clone();
        // Its delay hasn't passed yet.
        let waits = !props.delay.is_zero() && !avatar.delay_elapsed;
        Part::new(&avatar.instance, "fallback", None, avatar.state()).on_layout(
            move |layout, window, cx| {
                // Present while there's no loaded image, after its delay
                // (`present`). The image in the page this frame finds its
                // status here when the fallback lays out first, so a cached
                // one never shows the fallback, and an image that left gives
                // the fallback its place at once.
                let image = live.watch.borrow().upgrade();
                if let Some(image) = &image {
                    image.watch(window, cx);
                }
                let loaded = image.is_some() && live.status.get() == AvatarLoadingStatus::Loaded;
                layout.present(!waits && !loaded);
            },
        )
    }
}

/// `controlled: []`: the status is the image's doing, not a person's, so the
/// bridge reads no key of the avatar, and its api has no actions.
impl Agent for Avatar {
    const CONTROLLED: Option<&'static [&'static str]> = Some(&[]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
    }
}

/// The `image` part, as the app styles it: the part, holding the `img` that
/// fills it. Style the part like any div, and the image with
/// [`StyledImage`], like `object_fit`.
pub struct AvatarImage {
    part: Part<AvatarState>,
    img: Stateful<Img>,
    source: ImageSource,
    cache: Option<AnyImageCache>,
    avatar: WeakEntity<Avatar>,
    live: Rc<Live>,
}

impl AvatarImage {
    pub fn state(&self) -> &AvatarState {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(
        mut self,
        style: impl FnOnce(Stateful<Div>, &AvatarState) -> Stateful<Div>,
    ) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters, like `hidden` for an image a name
    /// beside it already names, where the original takes an empty `alt`.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.part = self.part.aria(aria);
        self
    }

    /// Changes the part with any of [`Part`]'s methods.
    pub fn map_part(mut self, map: impl FnOnce(Part<AvatarState>) -> Part<AvatarState>) -> Self {
        self.part = map(self.part);
        self
    }

    /// Loads the image through `cache`, as `img`'s `image_cache` does. An
    /// `image_cache` element around the avatar doesn't reach it, since the
    /// avatar loads its image itself.
    pub fn image_cache<I: ImageCache>(mut self, cache: &Entity<I>) -> Self {
        self.cache = Some(cache.clone().into());
        self
    }
}

impl Styled for AvatarImage {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl InteractiveElement for AvatarImage {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl StatefulInteractiveElement for AvatarImage {}

impl StyledImage for AvatarImage {
    fn image_style(&mut self) -> &mut ImageStyle {
        self.img.image_style()
    }
}

impl IntoElement for AvatarImage {
    type Element = PartElement;

    fn into_element(mut self) -> Self::Element {
        // gpui rounds an image as it paints it, and clips nothing to rounded
        // corners, so the image takes the part's.
        self.img.style().corner_radii = self.part.style().corner_radii.clone();
        let watch = Rc::new(ImageWatch {
            source: self.source,
            cache: self.cache,
            avatar: self.avatar,
            live: self.live.clone(),
            done: Cell::new(false),
        });
        *self.live.watch.borrow_mut() = Rc::downgrade(&watch);
        let live = self.live;
        self.part
            .child(self.img)
            .on_layout(move |layout, window, cx| {
                watch.watch(window, cx);
                // Until it loads, it's out of sight and out of the
                // accessibility tree, as of what it found this frame. The
                // hidden style goes on over the app's, as an inline style
                // wins over classes.
                if live.status.get() != AvatarLoadingStatus::Loaded {
                    layout.style().refine(&hidden_image_style());
                    layout.aria(Aria::new().hidden(true));
                }
            })
            .into_element()
    }
}

// What the image part watches, made each frame it renders and kept with its
// element while that frame draws, so the fallback can tell the image is in
// the page and ask for it first.
struct ImageWatch {
    source: ImageSource,
    cache: Option<AnyImageCache>,
    avatar: WeakEntity<Avatar>,
    live: Rc<Live>,
    // Whether it watched this frame, since the fallback may lay out first.
    done: Cell<bool>,
}

impl ImageWatch {
    // Follows the image: its status as it first draws, then each time it
    // loads or fails, and a new source starting over (`watchImage`).
    fn watch(&self, window: &mut Window, cx: &mut App) {
        if self.done.replace(true) {
            return;
        }
        let has_source = has_source(&self.source);
        let image = if !has_source {
            Some(Err(ImageCacheError::Asset(SharedString::new_static(
                "The image has no source.",
            ))))
        } else if let (ImageSource::Resource(resource), Some(cache)) = (&self.source, &self.cache) {
            cache.load(resource, window, cx)
        } else {
            use_image(&self.source, window, cx)
        };
        let status = image_status(image.as_ref(), has_source);
        let seen = Some((SourceKey::of(&self.source), status));
        {
            let mut watched = self.live.image.borrow_mut();
            match status {
                // `img` draws the last image that loaded until the next one has.
                AvatarLoadingStatus::Loading => watched.drawn = watched.loaded.clone().map(Ok),
                AvatarLoadingStatus::Loaded => {
                    watched.loaded = image
                        .as_ref()
                        .and_then(|image| image.as_ref().ok())
                        .cloned();
                    watched.drawn = image;
                }
                AvatarLoadingStatus::Idle | AvatarLoadingStatus::Error => {
                    watched.loaded = None;
                    watched.drawn = image;
                }
            }
            if watched.seen == seen {
                return;
            }
            watched.seen = seen;
        }
        let showing = self.live.status.get();
        let reason = match status {
            // A loaded image keeps showing while its next source loads, as
            // browsers draw it, so a new source never flashes the fallback.
            AvatarLoadingStatus::Loading if showing == AvatarLoadingStatus::Loaded => return,
            AvatarLoadingStatus::Loading => Reason::SrcChange,
            AvatarLoadingStatus::Loaded => Reason::ImageLoad,
            AvatarLoadingStatus::Idle | AvatarLoadingStatus::Error => Reason::ImageError,
        };
        if showing == status {
            return;
        }
        // The parts show it this frame, and its change reports it right after.
        self.live.status.set(status);
        let avatar = self.avatar.clone();
        window.defer(cx, move |window, cx| {
            avatar
                .update(cx, |avatar, cx| avatar.report(status, reason, window, cx))
                .ok();
        });
    }
}
