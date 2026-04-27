//! Custom mdast/hast plugins (Rust ports of zudo-doc/packages/md-plugins/).
//!
//! Each submodule contains one plugin (an [`MdastVisitor`] or
//! [`HastVisitor`] implementation). The `util` submodule holds shared
//! helpers used by multiple plugins.
//!
//! [`MdastVisitor`]: crate::pipeline::MdastVisitor
//! [`HastVisitor`]: crate::pipeline::HastVisitor

pub mod admonitions;
pub mod code_title;
pub mod directives;
pub mod heading_links;
pub mod image_enlarge;
pub mod mermaid;
pub mod resolve_links;
pub mod strip_md_ext;
pub mod syntect_plugin;
pub mod util;

pub use admonitions::{AdmonitionsPlugin, default_admonition_directives};
pub use code_title::CodeTitlePlugin;
pub use directives::{DirectiveDef, DirectiveDiagnostic, DirectiveKind, DirectiveRegistry};
pub use heading_links::HeadingLinksPlugin;
pub use image_enlarge::ImageEnlargePlugin;
pub use mermaid::MermaidPlugin;
pub use resolve_links::{ResolveLinksPlugin, ResolveMarkdownLinksOptions};
pub use strip_md_ext::StripMdExtensionPlugin;
pub use syntect_plugin::SyntectPlugin;
