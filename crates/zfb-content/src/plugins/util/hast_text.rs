//! Recursively extract text content from a hast tree.
//!
//! Moved to `zfb-md-ast::hast_text` so hast visitors in `zfb-md-extras`
//! (which cannot depend on `zfb-content`) can use it. Re-exported here so
//! all existing `crate::plugins::util::hast_text::extract_text` call sites
//! compile unchanged.

pub use zfb_md_ast::extract_text;
