// zfb-server live-reload client.
//
// Subscribes to the SSE stream at /__zfb/reload and reacts to three
// event types:
//
//   - "page":    full document reload (location.reload()).
//   - "css":     hot-swap every <link rel="stylesheet"> by appending
//                or updating a ?v=<timestamp> cache-busting query
//                string.
//   - "islands": dynamic import the named bundle URL and re-run the
//                hydration runtime so the affected component swaps in
//                place. Payload is a JSON object {component, bundleUrl}.
//
// EventSource auto-reconnects on transport errors; we only log the
// error so the developer knows something happened.
(function () {
  if (typeof window === "undefined" || typeof EventSource === "undefined") {
    return;
  }
  var src = new EventSource("/__zfb/reload");
  src.addEventListener("page", function () {
    window.location.reload();
  });
  src.addEventListener("css", function () {
    var ts = String(Date.now());
    var links = document.querySelectorAll('link[rel="stylesheet"]');
    for (var i = 0; i < links.length; i++) {
      var link = links[i];
      var href = link.getAttribute("href");
      if (!href) continue;
      var base = href.split("?")[0];
      link.setAttribute("href", base + "?v=" + ts);
    }
  });
  // Wire contract: component="" means "bundle changed, unknown components;
  // reload the whole bundle by re-importing bundleUrl". Must NOT short-circuit on empty component.
  src.addEventListener("islands", function (ev) {
    var payload;
    try {
      payload = JSON.parse(ev.data || "{}");
    } catch (e) {
      if (typeof console !== "undefined" && console.warn) {
        console.warn("[zfb] livereload: invalid islands payload", ev.data);
      }
      return;
    }
    var url = payload && payload.bundleUrl;
    if (!url) return;
    // Bust any module cache by appending a timestamp; the host page's
    // hydration runtime (zfb_islands::hydration_script_tag) re-runs
    // on import so re-importing the bundle re-hydrates the component
    // without a full page reload.
    var ts = String(Date.now());
    var swapUrl = url + (url.indexOf("?") >= 0 ? "&" : "?") + "v=" + ts;
    if (typeof window.__zfbIslandsReload === "function") {
      // Test/host hook so the runtime can intercept the swap-import
      // (e.g. to coordinate state preservation). When absent we fall
      // back to a plain dynamic import which re-runs the bundle's
      // top-level hydration calls.
      try {
        window.__zfbIslandsReload(payload.component, swapUrl);
        return;
      } catch (e) {
        if (typeof console !== "undefined" && console.warn) {
          console.warn("[zfb] livereload: islands hook threw", e);
        }
      }
    }
    // Dynamic import — supported in modern dev browsers. Compiled
    // through new Function so this file stays parseable as a classic
    // script (no top-level `import` keyword); the returned module's
    // top-level hydration code re-runs on import.
    try {
      new Function("u", "return import(u)")(swapUrl);
    } catch (e) {
      if (typeof console !== "undefined" && console.warn) {
        console.warn("[zfb] livereload: dynamic import failed", e);
      }
    }
  });
  src.addEventListener("error", function (ev) {
    // EventSource auto-reconnects; surface the event for visibility.
    if (typeof console !== "undefined" && console.warn) {
      console.warn("[zfb] livereload: connection error", ev);
    }
  });
})();
