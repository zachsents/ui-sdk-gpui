//! Port of `components/avatar/image-status.ts`.
//!
//! The original reads an `<img>`'s `complete` and `naturalWidth`. Here the
//! image is what `ui_sdk_image::use_image` gives for its source in the frame
//! it lays out, as `img` loads it: nothing while it loads, then the decoded
//! image or why it failed.

use std::sync::Arc;

use gpui::{ImageCacheError, RenderImage};

use crate::avatar::AvatarLoadingStatus;

/// What an image's status is right now, without waiting for an event. A
/// complete image either loaded or broke. One without a source has failed.
pub(crate) fn image_status(
    image: Option<&Result<Arc<RenderImage>, ImageCacheError>>,
    has_source: bool,
) -> AvatarLoadingStatus {
    if !has_source {
        return AvatarLoadingStatus::Error;
    }
    match image {
        None => AvatarLoadingStatus::Loading,
        // One that decoded to no pixels broke, as a `naturalWidth` of 0 says.
        Some(Ok(image)) if image.size(0).width.0 > 0 => AvatarLoadingStatus::Loaded,
        Some(_) => AvatarLoadingStatus::Error,
    }
}
