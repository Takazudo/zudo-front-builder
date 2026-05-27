//! Re-export shim: `MermaidPlugin` moved to `zfb-md-extras::mermaid` in Wave 3 (#570).
//!
//! All existing `zfb_content::plugins::mermaid::MermaidPlugin` import paths continue
//! to compile via this shim. New code should import from `zfb_md_extras::mermaid`
//! directly.

pub use zfb_md_extras::mermaid::MermaidPlugin;
