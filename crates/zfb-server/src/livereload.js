// zfb-server live-reload client.
//
// Subscribes to the SSE stream at /__zfb/reload and reacts to two
// event types:
//
//   - "page": full document reload (location.reload()).
//   - "css":  hot-swap every <link rel="stylesheet"> by appending or
//             updating a ?v=<timestamp> cache-busting query string.
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
  src.addEventListener("error", function (ev) {
    // EventSource auto-reconnects; surface the event for visibility.
    if (typeof console !== "undefined" && console.warn) {
      console.warn("[zfb] livereload: connection error", ev);
    }
  });
})();
