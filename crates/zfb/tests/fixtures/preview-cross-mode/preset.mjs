// Cross-mode acceptance fixture (issue #1547 / epic #1541 Preview Parity).
//
// `echoHandler` is registered VERBATIM under both `devMiddleware` and
// `previewMiddleware` — the whole point of `preview_cross_mode_e2e.rs` is
// to prove the same handler produces the same observable behaviour
// whether it is dispatched by `zfb dev` or `zfb preview` (static).
function echoHandler(req) {
  return {
    status: 200,
    headers: { "content-type": "text/plain; charset=utf-8" },
    body: `CROSS_MODE_PLUGIN_MARKER method=${req.method} url=${req.url}`,
  };
}

export default {
  name: "cross-mode-plugin",
  devMiddleware({ register }) {
    register("/api/echo", echoHandler);
  },
  previewMiddleware({ register }) {
    register("/api/echo", echoHandler);
  },
};
