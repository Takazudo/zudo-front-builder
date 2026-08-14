//! Custom mdast/hast plugins (Rust ports of zudo-doc/packages/md-plugins/).
//!
//! Each submodule contains one plugin (an [`MdastVisitor`] or
//! [`HastVisitor`] implementation). The `util` submodule holds shared
//! helpers used by multiple plugins.
//!
//! [`MdastVisitor`]: crate::pipeline::MdastVisitor
//! [`HastVisitor`]: crate::pipeline::HastVisitor

pub mod cjk_friendly;
pub mod code_title;
pub mod directives;
pub mod external_links;
pub mod hard_breaks;
pub mod heading_links;
pub mod mermaid;
pub mod resolve_links;
pub mod strip_md_ext;
pub mod syntect_plugin;
pub mod toc;
pub mod util;

// The GFM autolink boundary fix (#1105) and the nested-autolink unwrap
// (#2388) live in `zfb-md-ast`: both are post-parse normalisations that
// EVERY `markdown::to_mdast` call site must apply, including the ones in
// `zfb-md-extras` (transclude), which cannot depend on this crate.
pub use cjk_friendly::CjkFriendlyPlugin;
pub use code_title::CodeTitlePlugin;
pub use directives::{
    AttrSchema, AttrType, AttrValidationResult, DirectiveDef, DirectiveDiagnostic, DirectiveKind,
    DirectiveRegistry, ValidatedAttrValue,
};
pub use external_links::{ExternalLinksConfig, ExternalLinksPlugin};
pub use hard_breaks::HardBreaksPlugin;
pub use heading_links::HeadingLinksPlugin;
pub use mermaid::MermaidPlugin;
pub use resolve_links::{BrokenLinkDiagnostic, ResolveLinksPlugin, ResolveMarkdownLinksOptions};
pub use strip_md_ext::StripMdExtensionPlugin;
pub use syntect_plugin::SyntectPlugin;
pub use toc::{TocConfig, TocPlugin};
pub use zfb_md_ast::cjk_autolink::{self, CjkAutolinkBoundaryPlugin};
pub use zfb_md_ast::nested_link::{self, unwrap_nested_links};
