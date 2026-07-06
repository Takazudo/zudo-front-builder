/// <reference lib="dom" />
/// <reference lib="dom.iterable" />
// `@takazudo/zfb-runtime` — prefetch module.
//
// Ported from Astro's prefetch module (packages/astro/src/prefetch/index.ts).
// Source: https://github.com/withastro/astro
// Issue: Takazudo/zudo-front-builder#276
//
// Mechanical renames per W1B §13.5:
//   astro:* event names          → zfb:*
//   data-astro-prefetch          → data-zfb-prefetch
//   __PREFETCH_DISABLED__        → <meta name="zfb-prefetch-disabled" content="true">
//
// DISABLED-FLAG CONTRACT — exact meta tag name and content value are locked:
//   meta[name="zfb-prefetch-disabled"][content="true"]
// Both this module and sibling sub-issue #272-config pin this verbatim.

export type PrefetchStrategy = "hover" | "viewport" | "load" | "tap";

export interface PrefetchInitOptions {
  prefetchAll?: boolean;
  defaultStrategy?: PrefetchStrategy;
}

export interface PrefetchOptions {
  ignoreSlowConnection?: boolean;
  with?: "link" | "fetch";
}

// Module-level state — persists across SPA navigations within a session.
let initialized = false;
let prefetchAll = false;
let defaultStrategy: PrefetchStrategy = "hover";

// Set of hrefs that have been successfully prefetched.
const prefetched = new Set<string>();
// Map of in-flight prefetch promises for dedup (also used as the concurrent short-circuit guard).
const inFlight = new Map<string, Promise<void>>();

// How long to wait for a <link rel=prefetch>'s load/error before treating
// the prefetch as settled anyway (see executePrefetch).
const LINK_SETTLE_TIMEOUT_MS = 10_000;

// The viewport observer — retained across post-swap re-scans so new elements
// can be observed without recreating the observer.
let viewportObserver: IntersectionObserver | null = null;

// Hover cancel handle — set when a pointerenter idle-callback fires, cleared on
// pointerleave before the callback executes.
const hoverCancelHandles = new Map<Element, ReturnType<typeof setTimeout> | number>();

// Focus cancel handle — set when a focusin idle-callback fires, cleared on
// focusout before the callback executes.
const focusCancelHandles = new Map<Element, ReturnType<typeof setTimeout> | number>();

/**
 * Reset all module-level state. Exported for test isolation only — not part of
 * the public API and not re-exported from any barrel.
 */
export function __resetForTests(): void {
  initialized = false;
  prefetchAll = false;
  defaultStrategy = "hover";
  prefetched.clear();
  inFlight.clear();
  if (viewportObserver) {
    viewportObserver.disconnect();
    viewportObserver = null;
  }
  hoverCancelHandles.clear();
  focusCancelHandles.clear();
}

// ---------------------------------------------------------------------------
// Feature detection
// ---------------------------------------------------------------------------

function supportsLinkPrefetch(): boolean {
  const link = document.createElement("link");
  return link.relList?.supports?.("prefetch") ?? false;
}

function isSlowConnection(): boolean {
  const nav = navigator as Navigator & {
    connection?: { saveData?: boolean; effectiveType?: string };
  };
  return (
    nav.connection?.saveData === true ||
    nav.connection?.effectiveType === "2g" ||
    nav.connection?.effectiveType === "slow-2g"
  );
}

// ---------------------------------------------------------------------------
// Core prefetch logic
// ---------------------------------------------------------------------------

/**
 * Resolve an href to an absolute URL string keyed against the current origin.
 * Returns null for cross-origin hrefs (these are skipped entirely).
 */
function resolveHref(href: string): string | null {
  try {
    const url = new URL(href, location.href);
    if (url.origin !== location.origin) return null;
    return url.href;
  } catch {
    return null;
  }
}

/**
 * Execute a single prefetch for the given absolute href.
 * Uses <link rel="prefetch"> (preferred) or fetch() fallback.
 */
function executePrefetch(href: string, opts: PrefetchOptions): Promise<void> {
  const existing = inFlight.get(href);
  if (existing) return existing;

  const p: Promise<void> = (async () => {
    try {
      const method = opts.with ?? (supportsLinkPrefetch() ? "link" : "fetch");
      if (method === "link") {
        const link = document.createElement("link");
        link.rel = "prefetch";
        link.href = href;
        // Wait for load/error so we only mark success on actual load and allow
        // retry after error — inserting without waiting would mark success
        // immediately even on failure. (Bug: Takazudo/zudo-front-builder#896)
        // Some browsers (e.g. Safari) never fire load/error on rel=prefetch
        // links; without the settle timeout the href would sit in inFlight
        // forever and block every future retry. Timing out counts as
        // success — the resource was requested, which is all prefetch needs.
        await new Promise<void>((resolve, reject) => {
          const timer = setTimeout(() => resolve(), LINK_SETTLE_TIMEOUT_MS);
          link.addEventListener(
            "load",
            () => {
              clearTimeout(timer);
              resolve();
            },
            { once: true },
          );
          link.addEventListener(
            "error",
            () => {
              clearTimeout(timer);
              // Remove the failed element so retries don't accumulate
              // dead <link> nodes in <head>.
              link.remove();
              reject(new Error(`prefetch link error: ${href}`));
            },
            { once: true },
          );
          document.head.appendChild(link);
        });
      } else {
        await fetch(href, { priority: "low" } as RequestInit & { priority?: string });
      }
      prefetched.add(href);
    } finally {
      inFlight.delete(href);
    }
  })();

  inFlight.set(href, p);
  return p;
}

/**
 * Public prefetch function. Idempotent per href.
 */
export function prefetch(url: string, opts: PrefetchOptions = {}): void {
  const href = resolveHref(url);
  if (!href) return; // cross-origin — skip

  if (opts.ignoreSlowConnection !== true && isSlowConnection()) return;

  if (prefetched.has(href) || inFlight.has(href)) return;

  executePrefetch(href, opts).catch(() => {});
}

// ---------------------------------------------------------------------------
// Shared enter/leave handler factory
// ---------------------------------------------------------------------------

type CancelHandleMap = Map<Element, ReturnType<typeof setTimeout> | number>;

/**
 * Returns a pair of [enterHandler, leaveHandler] that queue and cancel an idle
 * prefetch callback, keyed on the supplied per-trigger cancel-handle map.
 *
 * Both the pointer (hover) and focus triggers use identical queue/cancel logic;
 * the only difference between them is which map tracks their pending handles.
 * Parameterising by the map eliminates that duplication.
 */
// pointerenter/pointerleave (and focusin/focusout) fire on every element the
// pointer/focus enters or leaves, including nested descendants — not just the
// delegated document listener's eventual `closest("a[href]")` target. For
// `<a><span>text</span></a>`, moving the pointer between the link's own
// children fires a leave on the child being left and an enter on the child
// being entered, even though the pointer never left the LINK itself.
// `relatedTarget` is where the pointer/focus is going (leave) or came from
// (enter); reading it lets the two handlers below tell an intra-link move
// apart from a real leave/first-enter. #1398.
type RelatedTargetEvent = Event & { relatedTarget?: EventTarget | null };

function makeEnterLeaveHandlers(
  cancelHandles: CancelHandleMap,
): [enterHandler: (e: Event) => void, leaveHandler: (e: Event) => void] {
  function enterHandler(e: Event): void {
    const link = (e.target as Element).closest("a[href]") as HTMLAnchorElement | null;
    if (!link) return;
    if (!shouldPrefetchLink(link, "hover")) return;

    // A handle is already queued for this link — the pointer/focus moved
    // between nested descendants of the SAME link, which re-fires enter on
    // each one. Keep the original debounce window instead of overwriting it
    // with a fresh requestIdleCallback/timeout that a busy pointer could keep
    // resetting indefinitely. #1398.
    if (cancelHandles.has(link)) return;

    // The pending debounce already FIRED for this link — its idle callback ran
    // prefetch(link.href), so the handle is gone from cancelHandles but the
    // href is now prefetched (or in-flight). A later enter on the SAME link
    // (e.g. an intra-link child->child move whose first callback fired in the
    // gap between the two pointer moves — which the synchronous-dispatch L2
    // suite cannot reproduce but a real browser does) must NOT queue a second,
    // redundant idle callback whose prefetch() would only no-op. This is the
    // fire+requeue companion to the cancel+requeue guard above. A failed
    // prefetch leaves the href in NEITHER set (executePrefetch deletes it from
    // inFlight without adding it to prefetched), so retry-after-error still
    // works. #1398.
    const resolvedHref = resolveHref(link.href);
    if (resolvedHref && (prefetched.has(resolvedHref) || inFlight.has(resolvedHref))) return;

    // Use requestIdleCallback if available, else a small timeout.
    const fire = () => {
      cancelHandles.delete(link);
      prefetch(link.href);
    };

    if (typeof requestIdleCallback !== "undefined") {
      const handle = requestIdleCallback(fire);
      cancelHandles.set(link, handle);
    } else {
      const handle = setTimeout(fire, 100);
      cancelHandles.set(link, handle);
    }
  }

  function leaveHandler(e: Event): void {
    const link = (e.target as Element).closest("a[href]") as HTMLAnchorElement | null;
    if (!link) return;

    // The pointer/focus is moving to relatedTarget. If that's still inside
    // the same link (a nested descendant), this is NOT a real leave — skip
    // the cancel so the pending prefetch survives intra-link moves. #1398.
    const related = (e as RelatedTargetEvent).relatedTarget;
    if (related instanceof Node && link.contains(related)) return;

    const handle = cancelHandles.get(link);
    if (handle === undefined) return;

    if (typeof cancelIdleCallback !== "undefined") {
      cancelIdleCallback(handle as number);
    } else {
      clearTimeout(handle as ReturnType<typeof setTimeout>);
    }
    cancelHandles.delete(link);
  }

  return [enterHandler, leaveHandler];
}

// ---------------------------------------------------------------------------
// Trigger: hover (pointerenter / pointerleave)
// ---------------------------------------------------------------------------

const [onPointerEnter, onPointerLeave] = makeEnterLeaveHandlers(hoverCancelHandles);

// ---------------------------------------------------------------------------
// Trigger: focus (focusin / focusout with cancel on focusout)
// ---------------------------------------------------------------------------

const [onFocusIn, onFocusOut] = makeEnterLeaveHandlers(focusCancelHandles);

// ---------------------------------------------------------------------------
// Trigger: tap (touchstart / mousedown)
// ---------------------------------------------------------------------------

function onTap(e: Event): void {
  const link = (e.target as Element).closest("a[href]") as HTMLAnchorElement | null;
  if (!link) return;
  if (!shouldPrefetchLink(link, "tap")) return;
  prefetch(link.href);
}

// ---------------------------------------------------------------------------
// Trigger: viewport (IntersectionObserver)
// ---------------------------------------------------------------------------

function initViewportObserver(): IntersectionObserver {
  if (viewportObserver) return viewportObserver;

  viewportObserver = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          const link = entry.target as HTMLAnchorElement;
          prefetch(link.href);
          // Once observed and in-flight, no need to keep observing.
          viewportObserver?.unobserve(link);
        }
      }
    },
    { threshold: 0 },
  );

  return viewportObserver;
}

function observeViewportLinks(): void {
  const observer = initViewportObserver();
  const selector =
    prefetchAll && defaultStrategy === "viewport"
      ? "a[href]:not([data-zfb-prefetch='false'])"
      : "a[data-zfb-prefetch='viewport']";

  document.querySelectorAll<HTMLAnchorElement>(selector).forEach((link) => {
    const href = resolveHref(link.href);
    if (href && !prefetched.has(href)) {
      observer.observe(link);
    }
  });
}

// ---------------------------------------------------------------------------
// Trigger: load (requestIdleCallback after DOMContentLoaded)
// ---------------------------------------------------------------------------

function prefetchLoadLinks(): void {
  const selector =
    prefetchAll && defaultStrategy === "load"
      ? "a[href]:not([data-zfb-prefetch='false'])"
      : "a[data-zfb-prefetch='load']";

  document.querySelectorAll<HTMLAnchorElement>(selector).forEach((link) => {
    const href = resolveHref(link.href);
    if (!href || prefetched.has(href)) return;

    if (typeof requestIdleCallback !== "undefined") {
      requestIdleCallback(() => prefetch(link.href));
    } else {
      setTimeout(() => prefetch(link.href), 0);
    }
  });
}

// ---------------------------------------------------------------------------
// Per-link strategy resolution
// ---------------------------------------------------------------------------

/**
 * Determine whether a given link should be prefetched for the supplied trigger strategy.
 * Returns false if:
 *   - data-zfb-prefetch="false" (always disabled)
 *   - The effective strategy for this link doesn't match the active trigger
 */
function shouldPrefetchLink(link: HTMLAnchorElement, triggerStrategy: PrefetchStrategy): boolean {
  const attr = link.dataset["zfbPrefetch"];

  // Explicitly disabled.
  if (attr === "false") return false;

  // Per-link explicit strategy.
  if (attr && attr !== "false") {
    return attr === triggerStrategy;
  }

  // No explicit attribute — rely on prefetchAll + defaultStrategy.
  if (!prefetchAll) return false;

  return defaultStrategy === triggerStrategy;
}

// ---------------------------------------------------------------------------
// Post-swap re-scan
// ---------------------------------------------------------------------------

function onAfterSwap(): void {
  // Disconnect the existing observer before re-scanning so detached anchors from
  // the old body are unobserved and don't accumulate across SPA swaps.
  // (Bug: Takazudo/zudo-front-builder#896 — observer leak across SPA swaps)
  if (viewportObserver) {
    viewportObserver.disconnect();
    viewportObserver = null;
  }
  observeViewportLinks();
  prefetchLoadLinks();
}

// ---------------------------------------------------------------------------
// init()
// ---------------------------------------------------------------------------

/**
 * Initialize the prefetch module.
 *
 * Idempotent — multiple calls are safe. The module-level `initialized` flag
 * ensures listeners are registered exactly once.
 *
 * DISABLED-FLAG CONTRACT: if `<meta name="zfb-prefetch-disabled" content="true">`
 * is present in the document at init() time, this function is a no-op.
 */
export function init(options?: PrefetchInitOptions): void {
  // Disabled-flag check — exact selector is the contract with #272-config.
  if (document.querySelector('meta[name="zfb-prefetch-disabled"][content="true"]')) {
    return;
  }

  if (initialized) return;
  initialized = true;

  prefetchAll = options?.prefetchAll ?? false;
  defaultStrategy = options?.defaultStrategy ?? "hover";

  // Hover + tap — document delegation, picks up SPA-inserted links automatically.
  document.addEventListener("pointerenter", onPointerEnter, { capture: true });
  document.addEventListener("pointerleave", onPointerLeave, { capture: true });
  // Focus — separate cancel map so hover and focus don't share handles.
  document.addEventListener("focusin", onFocusIn, { capture: true });
  document.addEventListener("focusout", onFocusOut, { capture: true });
  document.addEventListener("touchstart", onTap, { capture: true, passive: true });
  document.addEventListener("mousedown", onTap, { capture: true });

  // Viewport — observe current links immediately.
  observeViewportLinks();

  // Load — queue current links via idle callback.
  const queueLoad = () => prefetchLoadLinks();
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", queueLoad, { once: true });
  } else {
    queueLoad();
  }

  // Post-swap re-scan — re-walk viewport + load links after each SPA navigation.
  document.addEventListener("zfb:after-swap", onAfterSwap);
}
