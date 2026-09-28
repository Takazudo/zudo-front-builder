//! Preact adapter — the default zfb framework.
//!
//! Maps the [`Adapter`](super::Adapter) contract onto the Preact
//! ecosystem:
//!
//! - JSX import source: `"preact"` (drives SWC's automatic JSX runtime,
//!   producing `import { jsx, jsxs } from "preact/jsx-runtime"`).
//! - Render-to-string module: `"preact-render-to-string"`.

use super::Adapter;

/// Preact framework adapter.
///
/// Stateless — a single instance is reused across all page renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreactAdapter;

impl Adapter for PreactAdapter {
    fn name(&self) -> &'static str {
        "preact"
    }

    fn jsx_import_source(&self) -> &'static str {
        "preact"
    }

    fn render_to_string_module(&self) -> &'static str {
        "preact-render-to-string"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn name_jsx_source_and_render_module_are_stable() {
        let a = PreactAdapter;
        assert_eq!(a.name(), "preact");
        assert_eq!(a.jsx_import_source(), "preact");
        assert_eq!(a.render_to_string_module(), "preact-render-to-string");
    }
}
