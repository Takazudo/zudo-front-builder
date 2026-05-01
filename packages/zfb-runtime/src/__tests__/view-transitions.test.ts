// @vitest-environment happy-dom
/// <reference lib="dom" />
//
// The triple-slash above pulls DOM types into THIS FILE only — the
// package's tsconfig deliberately ships with `lib: ["ES2022"]` (the
// runtime is a Cloudflare Worker, no DOM there), but the smoke tests
// drive happy-dom and need `document`, `MouseEvent`, etc.
//
// Tests for `<ViewTransitions />` (#106 / closes #99).
//
// Two halves:
//   1. SSR snapshot — deterministic serialisation of the structural VNode
//      array the component returns. Pins the rendered head fragment so a
//      `view-transition` grep + a `document.startViewTransition` grep
//      cannot regress silently.
//   2. JSDOM smoke tests — driven by happy-dom (the workspace's standard
//      DOM stub; the spec said JSDOM but happy-dom is API-equivalent for
//      these click/event paths and is already a workspace dep). Each
//      "skipped link case" from the locked scope asserts that
//      `preventDefault` was NOT called on the click.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ViewTransitions } from "../view-transitions.js";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/**
 * Minimal stable serialiser for the structural VNode shape the component
 * returns. Mirrors the helper in `router.test.ts` but specialised for the
 * `<head>`-fragment shape: `dangerouslySetInnerHTML` is rendered as raw
 * inner HTML (this is the production behaviour of both Preact's and React's
 * SSR renderers; the runtime is intentionally framework-agnostic so the
 * test re-implements just enough of that contract to pin the bytes).
 */
function serialize(node: unknown): string {
  if (node === null || node === undefined || node === false) return "";
  if (typeof node === "string") return escapeHtml(node);
  if (typeof node === "number" || typeof node === "boolean") return String(node);
  if (Array.isArray(node)) return node.map(serialize).join("");
  if (typeof node === "object") {
    const v = node as { type?: unknown; props?: Record<string, unknown> };
    if (typeof v.type !== "string") return "";
    const props = v.props ?? {};
    const children = "children" in props ? props["children"] : undefined;
    const dangerous = props["dangerouslySetInnerHTML"] as { __html?: string } | undefined;
    const attrs = Object.keys(props)
      .filter((k) => k !== "children" && k !== "dangerouslySetInnerHTML")
      .sort()
      .map((k) => ` ${k}="${escapeHtml(String(props[k] ?? ""))}"`)
      .join("");
    if (dangerous && typeof dangerous.__html === "string") {
      return `<${v.type}${attrs}>${dangerous.__html}</${v.type}>`;
    }
    // Self-closing void elements like <meta> emit no closing tag in the
    // production renderers; for snapshot purposes we still emit
    // <meta ...></meta> deterministically — the meta tag's child is
    // `undefined` so the stringification collapses cleanly.
    return `<${v.type}${attrs}>${serialize(children)}</${v.type}>`;
  }
  return "";
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * Reach into the rendered VNode array and return the inline script source
 * exactly as the component would inject it. Used by the JSDOM smoke tests
 * to install the client router into the test document.
 */
function extractClientScript(): string {
  const fragment = ViewTransitions();
  const script = fragment[1] as { props: { dangerouslySetInnerHTML?: { __html?: string } } };
  const inner = script.props.dangerouslySetInnerHTML?.__html;
  if (typeof inner !== "string") {
    throw new Error("ViewTransitions did not produce an inline script body");
  }
  return inner;
}

/**
 * Install the client router into `document` and return a cleanup that
 * removes the listener. happy-dom does not run inline `<script>` text
 * automatically when a node is appended, so we evaluate the body via
 * `Function(...)`. The router's IIFE attaches a `click` listener to
 * `document` — that listener is what each smoke test below exercises.
 *
 * Vitest does not isolate the global `document` between tests in the same
 * file, so each `installRouter()` call would otherwise leak a listener
 * into the shared singleton and successive tests would see N listeners
 * fire per click. We spy on `document.addEventListener` while running
 * the script so we capture the exact callback, then return a cleanup
 * the caller invokes to remove it.
 */
function installRouter(): () => void {
  const src = extractClientScript();
  let captured: { type: string; listener: EventListener } | null = null;
  const original = document.addEventListener.bind(document);
  const spy = (type: string, listener: EventListener, options?: unknown) => {
    if (type === "click" && !captured) {
      captured = { type, listener };
    }
    return original(type, listener, options as AddEventListenerOptions | boolean | undefined);
  };
  (document as unknown as { addEventListener: typeof spy }).addEventListener = spy;
  try {
    // eslint-disable-next-line @typescript-eslint/no-implied-eval
    new Function(src)();
  } finally {
    (document as unknown as { addEventListener: typeof original }).addEventListener = original;
  }
  return () => {
    if (captured) {
      document.removeEventListener(captured.type, captured.listener);
      captured = null;
    }
  };
}

/**
 * Dispatch a `click` on the supplied anchor and report whether the
 * default action was prevented. The smoke tests use this to assert the
 * skipped-link cases NEVER preventDefault.
 *
 * `MouseEventInit` defaults: `button: 0` (primary), no modifier keys,
 * `bubbles: true`, `cancelable: true`. Per-test overrides supply the
 * skipped-link signal under test.
 */
function clickAnchor(anchor: HTMLAnchorElement, init?: MouseEventInit): boolean {
  const event = new MouseEvent("click", {
    bubbles: true,
    cancelable: true,
    button: 0,
    ...init,
  });
  anchor.dispatchEvent(event);
  return event.defaultPrevented;
}

// ---------------------------------------------------------------------------
// SSR snapshot
// ---------------------------------------------------------------------------

describe("ViewTransitions — SSR fragment", () => {
  it("returns a [meta, script] tuple", () => {
    const fragment = ViewTransitions();
    expect(Array.isArray(fragment)).toBe(true);
    expect(fragment).toHaveLength(2);
    expect((fragment[0] as { type: string }).type).toBe("meta");
    expect((fragment[1] as { type: string }).type).toBe("script");
  });

  it("renders the same-origin meta tag and an inline view-transition module script", () => {
    const html = serialize(ViewTransitions());
    // Meta tag — exact match. The `view-transition` content keyword is
    // load-bearing; the acceptance criterion grep depends on it.
    expect(html).toContain('<meta content="same-origin" name="view-transition">');
    // Module script — type=module guarantees deferred + strict mode.
    expect(html).toContain('<script type="module">');
    // Body must mention startViewTransition for the acceptance grep.
    expect(html).toContain("document.startViewTransition");
  });

  it("inline script body is well-formed JS (parses as a function)", () => {
    const src = extractClientScript();
    // A bare `new Function(src)` constructs a function body; if the
    // body itself is not valid JS, the constructor throws SyntaxError.
    expect(() => new Function(src)).not.toThrow();
  });

  it("inline script does not depend on any external module", () => {
    const src = extractClientScript();
    // No `import` keyword — the script must be fully self-contained
    // (zero added runtime dependencies, per acceptance criteria).
    expect(/(^|\s)import\s/.test(src)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Client-router smoke tests (happy-dom)
//
// Pattern: each test installs the router fresh into a clean document,
// builds an `<a>`, dispatches a click, and asserts on `defaultPrevented`.
// The router attaches a single `document`-scoped listener; happy-dom
// resets between tests via `afterEach` clearing the body.
// ---------------------------------------------------------------------------

describe("ViewTransitions — client router (happy-dom)", () => {
  let uninstall: (() => void) | null = null;

  beforeEach(() => {
    // Clean document state. happy-dom shares the same `document`
    // singleton across tests in a file, so each test installs the
    // listener fresh and the matching afterEach uninstalls it.
    document.body.innerHTML = "";
    // Hard-reset the URL. happy-dom actually performs a navigation when
    // a click on an `<a>` is not preventDefault'd, which is exactly
    // what every "skipped link case" test below produces — the result
    // is that `window.location.href` ends up as `mailto:...`,
    // `tel:...`, etc. Subsequent tests then resolve `new URL("/foo",
    // window.location.href)` against a non-http base, the listener's
    // same-origin check fails (the resolved URL.origin is `"null"`),
    // and the happy-path tests bail before reaching
    // `startViewTransition`. Using `window.location.href = ...` (not
    // `history.replaceState`) is the only happy-dom form that fully
    // restores `location.origin` after a non-http navigation.
    window.location.href = "http://localhost:3000/";
  });

  afterEach(() => {
    // Remove the listener captured by `installRouter` so it cannot
    // bleed through into the next test. Without this, repeated
    // `installRouter()` calls (one per test) would leave N listeners
    // attached to the shared `document` singleton, multiplying every
    // dispatched click and corrupting the per-test assertions.
    if (uninstall) {
      uninstall();
      uninstall = null;
    }
    delete (document as unknown as { startViewTransition?: unknown }).startViewTransition;
  });

  /** Helper: install the router and remember the cleanup. */
  function install(): void {
    uninstall = installRouter();
  }

  /**
   * Helper: append an anchor to body with the supplied attributes.
   */
  function makeAnchor(attrs: Record<string, string>): HTMLAnchorElement {
    const a = document.createElement("a");
    for (const [k, v] of Object.entries(attrs)) a.setAttribute(k, v);
    a.textContent = "click me";
    document.body.appendChild(a);
    return a;
  }

  // ---- skipped-link cases — preventDefault MUST NOT be called -----------

  it("does NOT intercept when document.startViewTransition is undefined", () => {
    // Default browser-without-VT-support state. The router must bail
    // before calling preventDefault so the default navigation runs.
    install();
    const a = makeAnchor({ href: "/about" });
    expect(clickAnchor(a)).toBe(false);
  });

  it("does NOT intercept modifier-key clicks (ctrl)", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/about" });
    expect(clickAnchor(a, { ctrlKey: true })).toBe(false);
  });

  it("does NOT intercept modifier-key clicks (meta/cmd)", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/about" });
    expect(clickAnchor(a, { metaKey: true })).toBe(false);
  });

  it("does NOT intercept modifier-key clicks (shift)", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/about" });
    expect(clickAnchor(a, { shiftKey: true })).toBe(false);
  });

  it("does NOT intercept modifier-key clicks (alt)", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/about" });
    expect(clickAnchor(a, { altKey: true })).toBe(false);
  });

  it("does NOT intercept middle-click (button !== 0)", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/about" });
    expect(clickAnchor(a, { button: 1 })).toBe(false);
  });

  it("does NOT intercept right-click (button !== 0)", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/about" });
    expect(clickAnchor(a, { button: 2 })).toBe(false);
  });

  it("does NOT intercept anchors with target=_blank", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/about", target: "_blank" });
    expect(clickAnchor(a)).toBe(false);
  });

  it("does NOT intercept anchors with a non-empty target other than _self", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/about", target: "named-frame" });
    expect(clickAnchor(a)).toBe(false);
  });

  it("does NOT intercept anchors with the download attribute", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/file.pdf", download: "" });
    expect(clickAnchor(a)).toBe(false);
  });

  it("does NOT intercept cross-origin hrefs", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "https://example.com/somewhere" });
    expect(clickAnchor(a)).toBe(false);
  });

  it("does NOT intercept hash-only same-page hrefs", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "#section-2" });
    expect(clickAnchor(a)).toBe(false);
  });

  it("does NOT intercept mailto: hrefs", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "mailto:foo@example.com" });
    expect(clickAnchor(a)).toBe(false);
  });

  it("does NOT intercept tel: hrefs", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "tel:+1-555-1234" });
    expect(clickAnchor(a)).toBe(false);
  });

  it("does NOT intercept javascript: hrefs", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    // happy-dom may throw on resolving javascript: hrefs; the router
    // bails on the scheme regex BEFORE constructing a URL, so this
    // should remain a no-op regardless.
    const a = makeAnchor({ href: "javascript:void(0)" });
    expect(clickAnchor(a)).toBe(false);
  });

  it("does NOT intercept already-default-prevented events", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const a = makeAnchor({ href: "/about" });
    // A pre-listener that consumes the event first. The router must
    // honour `event.defaultPrevented` and stay out of the way.
    document.addEventListener(
      "click",
      (e) => {
        e.preventDefault();
      },
      { once: true, capture: true },
    );
    expect(clickAnchor(a)).toBe(true); // prevented by the pre-listener
  });

  it("does NOT intercept clicks on non-anchor descendants without a parent <a>", () => {
    install();
    (
      document as unknown as { startViewTransition: (cb: () => void) => unknown }
    ).startViewTransition = () => ({});
    const button = document.createElement("button");
    document.body.appendChild(button);
    const event = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    button.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
  });

  // ---- happy-path: VT-supported, same-origin link is intercepted -----------

  it("intercepts a same-origin link and runs through document.startViewTransition", () => {
    install();
    const startVT = vi.fn((cb: () => void) => {
      // We do NOT actually run the callback (which assigns
      // location.href and would try to navigate happy-dom). Asserting
      // it was called is enough to prove the interception path.
      void cb;
      return { ready: Promise.resolve(), finished: Promise.resolve() };
    });
    (document as unknown as { startViewTransition: typeof startVT }).startViewTransition = startVT;

    const a = makeAnchor({ href: "/about" });
    const prevented = clickAnchor(a);

    expect(prevented).toBe(true);
    expect(startVT).toHaveBeenCalledTimes(1);
  });

  it("intercepts a click on a child element inside <a> (closest() resolution)", () => {
    install();
    const startVT = vi.fn(() => ({ ready: Promise.resolve(), finished: Promise.resolve() }));
    (document as unknown as { startViewTransition: typeof startVT }).startViewTransition = startVT;

    const a = makeAnchor({ href: "/about" });
    const span = document.createElement("span");
    span.textContent = "inner";
    a.appendChild(span);

    const event = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    span.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(startVT).toHaveBeenCalledTimes(1);
  });
});
