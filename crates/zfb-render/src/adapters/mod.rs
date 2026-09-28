//! Framework adapters.
//!
//! zfb supports the Preact JSX framework. The choice is made once at
//! config-load time via the `framework` field in `zfb.config.ts` and is
//! centralized through SWC's `transform-react` configuration — *not* per-file
//! pragmas.
//!
//! An [`Adapter`] supplies the JSX import source and the render-to-string
//! module used by the production SSR bundle.

pub mod preact;

pub use preact::PreactAdapter;

/// Which framework to render with. Preact is selected at config-load time.
///
/// Serde accepts the lowercase form so `zfb.config.ts` can spell the value
/// without surprise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framework {
    #[default]
    #[serde(alias = "preact")]
    Preact,
}

/// The portable, stateless framework adapter contract.
pub trait Adapter {
    /// Human-readable adapter name. Stable, lowercase, no whitespace.
    /// Used in error messages and in build logs.
    fn name(&self) -> &'static str;

    /// JSX import source to feed into SWC's `transform-react`
    /// `importSource` option. Drives both the JSX factory module and
    /// the automatic-runtime `jsx`/`jsxs` imports.
    fn jsx_import_source(&self) -> &'static str;

    /// Module specifier the JS runtime should resolve to obtain the
    /// synchronous render-to-string entry. The runtime resolver maps
    /// this specifier to an actual module load.
    fn render_to_string_module(&self) -> &'static str;
}

/// Construct the boxed adapter for a given [`Framework`].
///
/// This is the single dispatch point used by the rest of the crate;
/// callers should never instantiate adapters directly.
pub fn make_adapter(framework: Framework) -> Box<dyn Adapter> {
    match framework {
        Framework::Preact => Box::new(PreactAdapter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_default_is_preact() {
        assert_eq!(Framework::default(), Framework::Preact);
    }

    #[test]
    fn framework_deserializes_lowercase() {
        let f: Framework = serde_json::from_str("\"preact\"").unwrap();
        assert_eq!(f, Framework::Preact);
    }

    #[test]
    fn make_adapter_returns_correct_name() {
        assert_eq!(make_adapter(Framework::Preact).name(), "preact");
    }

    #[test]
    fn jsx_import_source_matches_framework() {
        assert_eq!(
            make_adapter(Framework::Preact).jsx_import_source(),
            "preact"
        );
    }

    #[test]
    fn render_to_string_module_matches_framework() {
        assert_eq!(
            make_adapter(Framework::Preact).render_to_string_module(),
            "preact-render-to-string"
        );
    }
}
