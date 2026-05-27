//! Re-export shim: `ImageEnlargePlugin` moved to `zfb-md-extras::image_enlarge`
//! in Wave 3 (#570).
//!
//! All existing `zfb_content::plugins::image_enlarge::ImageEnlargePlugin` import
//! paths continue to compile via this shim. New code should import from
//! `zfb_md_extras::image_enlarge` directly.

pub use zfb_md_extras::image_enlarge::ImageEnlargePlugin;
