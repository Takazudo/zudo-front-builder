// `@takazudo/zfb-runtime` — `<ViewTransitions />` component.
//
// Mirrors Astro's `<ViewTransitions />` integration (closes
// Takazudo/zudo-front-builder#99). Mounted by consumers in their layout
// `<head>`; never auto-injects.
//
// Renders TWO sibling JSX nodes (returned as an array, the structural
// equivalent of a fragment that both Preact and React render verbatim):
//
//   1. `<meta name="view-transition" content="same-origin">` — server-
//      rendered, not client-injected, so the browser sees it before any
//      script runs and there is no race window.
//   2. `<script type="module">` whose textContent is a self-invoking
//      client router. The router intercepts same-origin `<a>` clicks
//      and runs the navigation through `document.startViewTransition`
//      when the browser supports it; otherwise it falls back to the
//      browser's default full navigation. No SPA-style DOM swap — each
//      hop is still a real page load wrapped in a transition.
//
// JSX-runtime-agnostic: returns the same structural VNode shape used by
// `Island` (`{ type, props, key, constructor: undefined }`) so both
// `preact-render-to-string` and `react-dom/server` accept it without the
// runtime ever importing either framework.
//
// Scope explicitly locked (per #106):
//   - click interception only (no SPA swap)
//   - skipped link cases: modifier clicks, non-`_self` target, download
//     attr, cross-origin href, hash-only same-page href, mailto/tel/
//     javascript schemes, default-prevented events, form submissions
//   - history/back-forward: native browser behaviour
//   - prefetch: out of scope

/**
 * Public element shape — kept structural so consumers don't infer through
 * either framework's internal VNode type. Matches the shape `Island`
 * returns from `@takazudo/zfb`.
 */
export type ViewTransitionsElement = {
  readonly type: string;
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

/**
 * Client-side router source. Inlined into a `<script type="module">` text
 * node so there is no extra HTTP request and zero added runtime
 * dependencies in the package.
 *
 * Behaviour summary (matches the locked scope in #106):
 *
 *   - Listens for `click` events at `document` (capture: false). Anchor
 *     resolution uses `closest("a[href]")` to handle clicks landing on
 *     a child element of the link (icon, span, etc.).
 *   - Skips when the event was already default-prevented, when the
 *     mouse button isn't primary (covers middle/right via `button !== 0`),
 *     or when any modifier key (cmd/ctrl/shift/alt) is held — those
 *     gestures map to user-intentional new-tab / save-target affordances.
 *   - Skips when the anchor has `download`, when its `target` is non-
 *     empty and not `"_self"`, or when its href uses a non-navigable
 *     scheme (`mailto:` / `tel:` / `javascript:`). These match Astro's
 *     `<ViewTransitions />` integration so consumers migrating from
 *     Astro see the same set of intercepted links.
 *   - Skips cross-origin destinations (different `origin`).
 *   - Skips hash-only same-page jumps (same pathname + search, hash
 *     present) so in-page anchor navigation continues to scroll natively
 *     without a wasted transition.
 *   - When `document.startViewTransition` is undefined (every browser
 *     without VT support), the listener returns early WITHOUT calling
 *     `preventDefault`, letting the browser navigate normally.
 *   - When VT is supported, calls `preventDefault` and runs the
 *     navigation inside `document.startViewTransition(() => { location.href = url.href })`.
 *     The callback assigns `location.href` rather than calling
 *     `assign()` to match the simplest semantics (history entry pushed,
 *     back button works natively).
 */
const CLIENT_SCRIPT = `(() => {
  if (typeof document === "undefined") return;
  document.addEventListener("click", (event) => {
    if (event.defaultPrevented) return;
    if (event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const target = event.target;
    if (!target || typeof target.closest !== "function") return;
    const anchor = target.closest("a[href]");
    if (!anchor) return;
    if (anchor.hasAttribute("download")) return;
    const linkTarget = anchor.getAttribute("target");
    if (linkTarget && linkTarget !== "_self") return;
    const rawHref = anchor.getAttribute("href");
    if (!rawHref) return;
    if (/^(?:mailto|tel|javascript):/i.test(rawHref)) return;
    let url;
    try {
      url = new URL(anchor.href, window.location.href);
    } catch (_e) {
      return;
    }
    if (url.origin !== window.location.origin) return;
    if (
      url.pathname === window.location.pathname &&
      url.search === window.location.search &&
      url.hash
    ) {
      return;
    }
    if (typeof document.startViewTransition !== "function") return;
    event.preventDefault();
    document.startViewTransition(() => {
      window.location.href = url.href;
    });
  });
})();`;

/**
 * Internal VNode constructor. Mirrors `makeVNode` in `Island` — sets
 * `constructor: undefined` so `preact-render-to-string` recognises the
 * object as a structural VNode rather than as a foreign value (it would
 * otherwise emit nothing). The cast back to the public type keeps the
 * surface tight; the extra runtime field is JS-only.
 */
function makeVNode(shape: {
  type: string;
  props: Record<string, unknown>;
  key: unknown;
}): ViewTransitionsElement {
  const v = shape as unknown as { constructor?: unknown };
  v.constructor = undefined;
  return shape as ViewTransitionsElement;
}

/**
 * `<ViewTransitions />` — server-renders a `<meta name="view-transition" content="same-origin">`
 * tag plus an inline `<script type="module">` that registers the
 * click-intercepting client router.
 *
 * Mount in the layout `<head>`; consumer code looks like:
 *
 *   <head>
 *     <ViewTransitions />
 *     ...
 *   </head>
 *
 * No auto-injection: the optional `viewTransitions: true` config flag
 * from #99 is deferred to a follow-up issue.
 *
 * Returns a `readonly [meta, script]` tuple — both Preact and React's
 * SSR renderers accept an array of VNodes as a fragment.
 */
export function ViewTransitions(): readonly ViewTransitionsElement[] {
  const meta = makeVNode({
    type: "meta",
    props: {
      name: "view-transition",
      content: "same-origin",
    },
    key: null,
  });

  const script = makeVNode({
    type: "script",
    props: {
      type: "module",
      // `dangerouslySetInnerHTML` is the only cross-framework-safe way
      // to inject literal script text without HTML-escaping the `<` and
      // `&` operators inside the body. Both Preact and React accept the
      // `{__html: string}` shape (Preact treats it as a structural alias).
      // Using `children` instead would HTML-escape the body and break
      // every comparison/regex in the client router.
      dangerouslySetInnerHTML: { __html: CLIENT_SCRIPT },
    },
    key: null,
  });

  return [meta, script];
}
