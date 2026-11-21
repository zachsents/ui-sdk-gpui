//! Port of `components/avatar`: an image, with a fallback while it loads or
//! if it fails.
//!
//! # TypeScript file → Rust module
//!
//! `avatar.ts` → [`avatar`], `image-status.ts` → `image_status`.
//!
//! # gpui
//!
//! The original follows a real `<img>`: its status as it binds, then its
//! `load` and `error` events, and a `MutationObserver` on its source. gpui's
//! `img` tells nothing of the kind, so the image part asks for its source the
//! way `img` does, through `ui_sdk_image::use_image` and the same caches, in
//! each frame it lays out. A first answer, a new answer or a new source is
//! what the original's events said. The part hands `img` what to draw, so a
//! loaded image keeps showing while a new source loads, as browsers draw it.
//!
//! The parts show what the image found in the frame it found it, and the
//! change reports it right after the frame, since a callback that ran during
//! the draw couldn't ask for the next one. A cached image is visible, and
//! the fallback gone, in the first frame, as the original's are before the
//! first paint, and a fallback takes the place of an image in the frame the
//! image leaves. So the image and the fallback decide as they lay out
//! ([`Part::on_layout`](ui_sdk_kernel::Part::on_layout)): the image hides
//! itself, from sight and from the accessibility tree, and the fallback
//! comes and goes. Whichever lays out first asks for the image, so the
//! fallback knows what it found even when it comes first. The image is its
//! own type, [`AvatarImage`], around a [`Part`](ui_sdk_kernel::Part), since
//! it holds gpui's `img`, which the app styles with `StyledImage`.
//!
//! The root and the image coming and going are the kernel's bound parts,
//! which notify the avatar: the root's arrival starts the fallback's delay
//! (its `bind`), and the image leaving reports `Idle` (`watchImage`'s cleanup
//! and `watchImageRemoval`), as does the avatar's entity going while its
//! image is in the page. `data-loading-status` is
//! [`AvatarState::loading_status`].
//!
//! # Deviations
//!
//! - `details.event` is always `None`: gpui's `img` has no `load` or `error`
//!   event.
//! - The parts show a new status a frame before `with` and `state()` do,
//!   which read the one that stands, and a status `details.cancel()` refuses
//!   goes back a frame after it showed. The original's store has it before
//!   anything renders.
//! - The image part is a div holding `img`, which fills it, where the
//!   original's `<img>` is the part. Size the part, and round it to round the
//!   image, since gpui clips nothing to rounded corners. `alt` is the part's
//!   `aria_label`.
//! - An `image_cache` element around the avatar doesn't reach its image,
//!   which the avatar loads itself: pass the cache with
//!   [`AvatarImage::image_cache`].
//! - `pointer-events: none` is gpui's `invisible`, which paints nothing and
//!   takes no pointer events.
//! - `AvatarLoadingStatusChangeReason` is the shared
//!   [`Reason`](ui_sdk_kernel::Reason)'s `ImageLoad`, `ImageError` and
//!   `SrcChange`. `AvatarPrimitive`, the definition's type, is the [`Avatar`]
//!   struct.
//!
//! # Dropped, and why
//!
//! - `fallbackDelay`, the option web components pass because their fallback
//!   may upgrade after the avatar binds. A gpui app builds the fallback in
//!   the same render as the root, so its own `delay` is there to read.
//! - The image in server markup, fetched before hydration: there's no server
//!   rendering.
//! - The image's `srcSet`, `sizes`, `crossOrigin`, `referrerPolicy` and
//!   `loading="lazy"`: gpui's `img` takes one source and loads it as it lays
//!   out.

pub mod avatar;
mod image_status;

pub use avatar::{
    Avatar, AvatarFallbackProps, AvatarImage, AvatarLoadingStatus, AvatarOptions, AvatarState,
};
