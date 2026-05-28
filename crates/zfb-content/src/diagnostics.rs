//! Generic markdown diagnostics sink for wave-6 visitors.
//!
//! The types moved into `zfb-md-ast::diagnostics` so the visitor contract
//! (`HastVisitor::visit_with_context`) can reference them without
//! `zfb-content` depending on `zfb-md-extras`. This module re-exports the
//! moved API under its historical path for backwards compatibility.

pub use zfb_md_ast::diagnostics::*;
