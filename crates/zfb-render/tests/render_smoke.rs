//! Smoke tests for the zfb-render skeleton.
//!
//! These exercise the SWC pipeline end-to-end and prove the `Renderer`
//! orchestration plumbs everything correctly. We swap in a `TestHost` for
//! the JS runtime side because the production host (per ADR-005, a
//! miniflare subprocess client wired in by the build orchestrator in T6)
//! is not in-process and would needlessly slow these unit-shape tests.
//!
//! The test host inspects the compiled JS that SWC produced, confirms the
//! transform did its job (TS stripped, JSX desugared into `jsx(...)` calls,
//! synthetic `<source>/jsx-runtime` import injected), and emits the expected
//! HTML the way a real adapter eventually will.

use serde_json::Value as JsonValue;
use zfb_render::{JsxRuntime, ModuleHandle, RenderHost, RenderRequest, Renderer, Result};

/// In-process host that asserts the SWC output looks transformed and then
/// returns a fixed render-to-string result. Plays the role Sub 4's adapter
/// + Sub 1's runtime will play once both land.
struct TestHost {
    last_module: Option<(ModuleHandle, String)>,
}

impl TestHost {
    fn new() -> Self {
        Self { last_module: None }
    }
}

impl RenderHost for TestHost {
    fn execute_module(&mut self, name: &str, source: &str) -> Result<ModuleHandle> {
        let handle =
            ModuleHandle::new(self.last_module.as_ref().map_or(0, |(h, _)| h.id + 1), name);
        self.last_module = Some((handle.clone(), source.to_string()));
        Ok(handle)
    }

    fn call_default(&mut self, _handle: &ModuleHandle, _props: JsonValue) -> Result<String> {
        let (_h, src) = self
            .last_module
            .as_ref()
            .expect("execute_module must be called first");
        // Verify SWC actually transformed: synthetic JSX import landed and the
        // raw JSX literal is gone.
        assert!(
            src.contains("preact/jsx-runtime"),
            "expected preact/jsx-runtime import in compiled output, got:\n{src}"
        );
        assert!(
            !src.contains("<div>hello smoke</div>"),
            "JSX should have been desugared, got:\n{src}"
        );
        Ok("<div>hello smoke</div>".to_string())
    }

    fn get_export(&mut self, _handle: &ModuleHandle, _name: &str) -> Result<JsonValue> {
        Ok(JsonValue::Null)
    }
}

#[test]
fn renders_tiny_tsx_page_to_html() {
    let host = TestHost::new();
    let mut renderer = Renderer::new(host, JsxRuntime::Preact);

    let req = RenderRequest::new(
        "pages/smoke.tsx",
        "export default function Page(): string {\n  return <div>hello smoke</div>;\n}\n",
    );

    let html = renderer.render(&req).expect("render ok");
    assert!(
        html.contains("<div>"),
        "expected output to contain <div>, got: {html}"
    );
    assert!(
        html.contains("hello smoke"),
        "expected output to contain greeting, got: {html}"
    );
}

#[test]
fn cache_is_populated_after_render() {
    let host = TestHost::new();
    let mut renderer = Renderer::new(host, JsxRuntime::Preact);

    let req = RenderRequest::new(
        "pages/smoke.tsx",
        "export default function Page() { return <span/>; }\n",
    );

    let _ = renderer.render(&req).expect("render ok");
    assert_eq!(renderer.loader().cache_len(), 1);
}
