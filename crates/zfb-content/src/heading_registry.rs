//! Cross-file heading-ID registry for wave-6 link validation.
//!
//! The types moved into `zfb-md-ast::heading_registry` so the visitor
//! contract (`HastVisitor::visit_with_context` / `BuildContext`) can
//! reference them without `zfb-content` depending on `zfb-md-extras`.
//! This module re-exports the moved API under its historical path for
//! backwards compatibility.

pub use zfb_md_ast::heading_registry::*;
