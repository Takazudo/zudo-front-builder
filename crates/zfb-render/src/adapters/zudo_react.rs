//! Adapter for zfb's owned JSX runtime.

use super::Adapter;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZudoReactAdapter;

impl Adapter for ZudoReactAdapter {
    fn name(&self) -> &'static str {
        "zudo-react"
    }

    fn jsx_import_source(&self) -> &'static str {
        "@takazudo/zfb/zudo-react"
    }

    fn render_to_string_module(&self) -> &'static str {
        "@takazudo/zfb/zudo-react/server"
    }
}
