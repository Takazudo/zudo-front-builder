//! Custom mdast/hast plugins (Rust ports of zudo-doc/packages/md-plugins/).
//!
//! Each submodule contains one plugin (an [`MdastVisitor`] or
//! [`HastVisitor`] implementation). The `util` submodule holds shared
//! helpers used by multiple plugins.
//!
//! [`MdastVisitor`]: crate::pipeline::MdastVisitor
//! [`HastVisitor`]: crate::pipeline::HastVisitor

pub mod cjk_autolink;
pub mod cjk_friendly;
pub mod code_title;
pub mod directives;
pub mod external_links;
pub mod hard_breaks;
pub mod heading_links;
pub mod mermaid;
pub mod nested_link;
pub mod resolve_links;
pub mod strip_md_ext;
pub mod syntect_plugin;
pub mod toc;
pub mod util;

pub use cjk_autolink::CjkAutolinkBoundaryPlugin;
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
pub use nested_link::unwrap_nested_links;
pub use resolve_links::{BrokenLinkDiagnostic, ResolveLinksPlugin, ResolveMarkdownLinksOptions};
pub use strip_md_ext::StripMdExtensionPlugin;
pub use syntect_plugin::SyntectPlugin;
pub use toc::{TocConfig, TocPlugin};
