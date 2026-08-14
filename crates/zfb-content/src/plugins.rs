//! Custom mdast/hast plugins (Rust ports of zudo-doc/packages/md-plugins/).
//!
//! Each submodule contains one plugin (an [`MdastVisitor`] or
//! [`HastVisitor`] implementation). The `util` submodule holds shared
//! helpers used by multiple plugins.
//!
//! [`MdastVisitor`]: crate::pipeline::MdastVisitor
//! [`HastVisitor`]: crate::pipeline::HastVisitor

pub mod code_title;
pub mod directives;
pub mod external_links;
pub mod heading_links;
pub mod mermaid;
pub mod resolve_links;
pub mod strip_md_ext;
pub mod syntect_plugin;
pub mod toc;
pub mod util;

// The GFM autolink boundary fix (#1105), the nested-autolink unwrap
// (#2388), and — since #2398 — the CJK-friendly emphasis pass and the
// hard-breaks pass all live in `zfb-md-ast`: each is a post-parse
// normalisation that a `markdown::to_mdast` call site owns for the tree
// it produces, since nothing downstream re-runs it. That includes the
// secondary parse sites in `zfb-md-extras` (transclude) and this crate's
// own `DirectiveRegistry::reparse_block`, which need to apply these
// passes to a subtree parsed from *inside* the mdast visitor chain —
// invisible to the fixed-chain-index copies `Pipeline` wires into its own
// chain (zfb#2390 / zfb#1105 / zfb#2398). Sites that deliberately opt out
// document why (see `facade.rs`'s raw-dialect entry point).
pub use code_title::CodeTitlePlugin;
pub use directives::{
    AttrSchema, AttrType, AttrValidationResult, DirectiveDef, DirectiveDiagnostic, DirectiveKind,
    DirectiveRegistry, ValidatedAttrValue,
};
pub use external_links::{ExternalLinksConfig, ExternalLinksPlugin};
pub use heading_links::HeadingLinksPlugin;
pub use mermaid::MermaidPlugin;
pub use resolve_links::{BrokenLinkDiagnostic, ResolveLinksPlugin, ResolveMarkdownLinksOptions};
pub use strip_md_ext::StripMdExtensionPlugin;
pub use syntect_plugin::SyntectPlugin;
pub use toc::{TocConfig, TocPlugin};
pub use zfb_md_ast::cjk_autolink::{self, CjkAutolinkBoundaryPlugin};
pub use zfb_md_ast::cjk_friendly::{self, CjkFriendlyPlugin};
pub use zfb_md_ast::hard_breaks::{self, HardBreaksPlugin};
pub use zfb_md_ast::nested_link::{self, unwrap_nested_links};
