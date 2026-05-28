//! Re-export shim: `TocPlugin` moved to `zfb-md-extras::heading_marker_toc`
//! in Wave 3 (#570). `TocConfig` lives in `zfb-md-ast`.
//!
//! All existing `zfb_content::plugins::toc::{TocConfig, TocPlugin}` import
//! paths continue to compile via this shim. New code should import from
//! `zfb_md_extras::heading_marker_toc` directly.

pub use zfb_md_ast::TocConfig;
pub use zfb_md_extras::heading_marker_toc::TocPlugin;
