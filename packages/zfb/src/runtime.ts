// Hydration scheduling helper consumed by the hydration runtime (Sub 3).
//
// Sub 3 owns the hydration runtime that walks the DOM, finds elements
// marked with `data-zfb-island`, and dispatches each one through this
// helper to decide *when* to fire the actual hydrate() call. The helper
// itself does not know how to hydrate — it only schedules the supplied
// `fire` callback.
//
// The branching matches the `When` union exactly:
//   "visible" → IntersectionObserver, threshold 0.0, hydrate on first
//                intersection, then disconnect.
//   "idle"    → requestIdleCallback if available, otherwise setTimeout(0).
//   "media"   → matchMedia(target's data-media), hydrate when the query
//                first matches (now or on a later change event), then
//                remove the listener.
//   "load"    → immediate, synchronous fire.
//
// Anything else is treated as "load" (with a console.warn in development).
// The helper is environment-tolerant: callers can run it in jsdom /
// happy-dom or bare Node, and the absence of `IntersectionObserver` /
// `requestIdleCallback` / `matchMedia` is handled gracefully.

import { resolveWhen, type When } from "./types.js";

/**
 * Subset of the global object that this module touches. Cast once at the
 * module top so individual scheduler functions don't repeat the inline
 * widening.
 */
type SchedulerGlobal = typeof globalThis & {
  requestIdleCallback?: (
    cb: (deadline: { didTimeout: boolean; timeRemaining: () => number }) => void,
    options?: { timeout?: number },
  ) => number;
  cancelIdleCallback?: (handle: number) => void;
  IntersectionObserver?: typeof IntersectionObserver;
  matchMedia?: typeof matchMedia;
};

const g = globalThis as SchedulerGlobal;

/**
 * Internal variant of `scheduleHydrate` that also reports whether the fire
 * callback was invoked synchronously. Unexported — call sites in this module
 * use this to decide whether to register a `pendingCancels` entry.
 */
function scheduleHydrateInternal(
  target: Element,
  when: When | string | undefined,
  fire: () => void,
): { fired: boolean; cancel: () => void } {
  const resolved = resolveWhen(when);

  if (resolved === "load") {
    fire();
    return { fired: true, cancel: noop };
  }

  if (resolved === "idle") {
    return { fired: false, cancel: scheduleIdle(fire) };
  }

  if (resolved === "media") {
    return scheduleMedia(target, fire);
  }

  // "visible"
  return scheduleVisible(target, fire);
}

/**
 * Schedule a hydration `fire` callback for `target` according to `when`.
 *
 * Returns a `cancel` function that aborts the scheduling if it has not
 * fired yet. After firing, calling `cancel` is a no-op. If the helper
 * cannot find the relevant browser API (e.g. running in pure Node with no
 * polyfill), it falls back to firing synchronously so server-side smoke
 * tests still observe the call.
 */
export function scheduleHydrate(
  target: Element,
  when: When | string | undefined,
  fire: () => void,
): () => void {
  return scheduleHydrateInternal(target, when, fire).cancel;
}

function noop(): void {
  // intentionally empty
}

/**
 * Build a one-shot gate around `fn`. The returned `run` invokes `fn`
 * exactly once provided `cancel` has not been called first; `cancel`
 * marks the gate as cancelled (later `run` invocations become no-ops)
 * and reports whether the gate had already fired.
 */
function oneShot(fn: () => void): {
  run: () => void;
  cancel: () => boolean;
} {
  let fired = false;
  let cancelled = false;
  return {
    run(): void {
      if (cancelled || fired) return;
      fired = true;
      fn();
    },
    cancel(): boolean {
      if (fired) return true;
      cancelled = true;
      return false;
    },
  };
}

function scheduleIdle(fire: () => void): () => void {
  const gate = oneShot(fire);

  if (typeof g.requestIdleCallback === "function") {
    const handle = g.requestIdleCallback(gate.run);
    return () => {
      const alreadyFired = gate.cancel();
      if (alreadyFired) return;
      if (typeof g.cancelIdleCallback === "function") g.cancelIdleCallback(handle);
    };
  }

  const handle = setTimeout(gate.run, 0);
  return () => {
    const alreadyFired = gate.cancel();
    if (alreadyFired) return;
    clearTimeout(handle);
  };
}

function scheduleVisible(
  target: Element,
  fire: () => void,
): { fired: boolean; cancel: () => void } {
  const Observer = g.IntersectionObserver;

  // No IntersectionObserver (e.g. very old browsers, bare Node) — fail
  // open and hydrate immediately so the island is at least functional.
  if (typeof Observer !== "function") {
    fire();
    return { fired: true, cancel: noop };
  }

  const gate = oneShot(fire);
  const observer = new Observer(
    (entries, obs) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          obs.disconnect();
          gate.run();
          return;
        }
      }
    },
    { threshold: 0 },
  );

  observer.observe(target);

  return {
    fired: false,
    cancel: () => {
      const alreadyFired = gate.cancel();
      if (alreadyFired) return;
      observer.disconnect();
    },
  };
}

function scheduleMedia(target: Element, fire: () => void): { fired: boolean; cancel: () => void } {
  const query = target.getAttribute("data-media");

  // No matchMedia API (e.g. bare Node, very old browser) or missing/empty
  // query — fail open and hydrate immediately so the island is at least
  // functional.
  if (typeof g.matchMedia !== "function" || !query) {
    fire();
    return { fired: true, cancel: noop };
  }

  const mql = g.matchMedia(query);

  // Already matches — fire synchronously (no pending listener needed).
  if (mql.matches) {
    fire();
    return { fired: true, cancel: noop };
  }

  const gate = oneShot(fire);

  let removeListener = noop;

  // Listen for the first change event where the query matches.
  // We do NOT use `{once:true}` because we must ignore un-match events
  // (e.g. viewport widens back above breakpoint) and only fire on the
  // first match event — `{once:true}` would consume any change, including
  // un-match changes.
  const handler = (e: MediaQueryListEvent): void => {
    if (!e.matches) return; // ignore un-match changes
    removeListener();
    gate.run();
  };

  // Modern browsers expose the EventTarget API on MediaQueryList; older
  // Safari (<14) only has the deprecated addListener/removeListener pair
  // and throws on addEventListener. Prefer modern, fall back to legacy,
  // and fail open when neither exists (mirrors the missing-matchMedia case).
  if (typeof mql.addEventListener === "function") {
    mql.addEventListener("change", handler);
    removeListener = () => mql.removeEventListener("change", handler);
  } else if (typeof mql.addListener === "function") {
    mql.addListener(handler);
    removeListener = () => mql.removeListener(handler);
  } else {
    fire();
    return { fired: true, cancel: noop };
  }

  return {
    fired: false,
    cancel: () => {
      const alreadyFired = gate.cancel();
      if (alreadyFired) return;
      removeListener();
    },
  };
}

// ---------------------------------------------------------------------------
// mountIslands — DOM walk + dynamic-import dispatcher.
//
// `mountIslands` is the entry point the generated `islands-runtime-<hash>.js`
// bundle calls at script load time. It walks the DOM for the two island
// markers emitted by the server-side hydration step and the `<Island>`
// JSX wrapper:
//
//   1. `[data-zfb-island]` — SSR'd islands. We `hydrate()` (Preact) /
//      `hydrateRoot()` (React) against the existing server-rendered
//      DOM, gated by `scheduleHydrate(when)`.
//
//   2. `[data-zfb-island-skip-ssr]` — SSR-skip islands. The server
//      emitted no markup for these, so we `render()` (Preact) /
//      `createRoot().render()` (React). Skipping hydrate for this case
//      avoids the hydrate-mismatch warnings React/Preact would emit
//      against an empty DOM container.
//
// The shared bundle supplies each island's mount module inline. The
// framework-specific glue is built into that bundle.
// ---------------------------------------------------------------------------

/**
 * The mount function supplied by a shared-bundle island module.
 *
 * `mode === "hydrate"` is used for SSR'd islands, `"render"` for
 * SSR-skip islands.
 */
type IslandMount = (
  props: Record<string, unknown>,
  element: Element,
  mode: "hydrate" | "render",
) => void | IslandRootHandle | null;

interface IslandRootHandle {
  dispose(): void;
  unmount?(): void;
  readonly disposed?: boolean;
}

type IslandUnmount = (element: Element) => void;

interface IslandModule {
  mount?: IslandMount;
  default?: IslandMount;
  unmount?: IslandUnmount;
}

/**
 * Map of `componentName → island descriptor` baked into the runtime entry.
 *
 * Each manifest value is an inline module with `mount` or `default`.
 * The shared bundle imports island sources at build time.
 */
export type IslandManifestValue = IslandModule;
export type IslandManifest = Readonly<Record<string, IslandManifestValue>>;

// data-zfb-transition-persist marker attribute — the client-router's persist
// contract. Mirrored from client-router/swap-functions.ts: that package owns the
// body swap (lifting persisted nodes into the incoming body), this package owns
// island mount/unmount. Both must agree on the literal string. See the port
// spec at packages/zfb-runtime/docs/client-router/port-spec.md §12.3.
const PERSIST_ATTR = "data-zfb-transition-persist";

// Cross-package "needs-remount" flag set by client-router/swap-functions.ts on a
// persisted island whose props changed across a body swap. Mirrored literal (same
// cross-package contract as PERSIST_ATTR above — both packages must agree on the
// string). Consumed by clearMountedForRemount(). See #1389.
const ISLAND_REMOUNT_ATTR = "data-zfb-island-remount";

/**
 * Public observation marker; the symbol handle is the live-root guard.
 *
 * State table:
 * - initial/deferred/missing entry/failed mount: marker and handle absent;
 * - successful mount: both present;
 * - discarded root: disposal leaves DOM for the body swap, then clears both;
 * - unchanged persisted root: both survive with the same DOM node;
 * - changed persisted root: dispose, clear, then mount in render mode;
 * - bundle re-import: dispose the old symbol handle before render mode replaces it.
 */
export const ISLAND_MOUNTED_ATTR = "data-zfb-island-mounted";
const ROOT_KEY = Symbol.for("@takazudo/zfb/zudo-react/root-v1");
const COMPOSITION_KEY = Symbol.for("@takazudo/zfb/zudo-react/composition-v1");
const TRACKER_KEY = Symbol.for("@takazudo/zfb/zudo-react/composition-tracker-v1");

// A fresh bundle instance replaces roots installed by a previous instance.
// Ordinary repeat scans in the same instance keep their live handles.
const owned = new WeakSet<Element>();
function rootHandle(element: Element): IslandRootHandle | undefined {
  return (element as unknown as Record<symbol, IslandRootHandle | undefined>)[ROOT_KEY];
}
function setRootHandle(element: Element, handle: IslandRootHandle | undefined): void {
  const target = element as unknown as Record<symbol, IslandRootHandle | undefined>;
  if (handle) target[ROOT_KEY] = handle;
  else delete target[ROOT_KEY];
}
function reportIslandError(element: Element, phase: string, error: unknown): void {
  const name =
    element.getAttribute("data-zfb-island") ??
    element.getAttribute("data-zfb-island-skip-ssr") ??
    "unknown";
  try {
    console.error(`[zfb] island "${name}" ${phase} failed`, error);
  } catch {
    // A broken reporter must not stop other roots.
  }
}
function disposeIsland(element: Element, phase: string): void {
  const handle = rootHandle(element);
  try {
    handle?.dispose();
  } catch (error) {
    reportIslandError(element, phase, error);
  } finally {
    if (rootHandle(element) === handle) setRootHandle(element, undefined);
    element.removeAttribute(ISLAND_MOUNTED_ATTR);
  }
}
function installCompositionTracker(doc: Document): void {
  const target = doc as unknown as Record<symbol, (() => void) | undefined>;
  if (target[TRACKER_KEY]) return;
  const set = (event: Event, value: "active" | "idle") => {
    if (event.target instanceof Element) {
      (event.target as unknown as Record<symbol, string>)[COMPOSITION_KEY] = value;
    }
  };
  const start = (event: Event) => set(event, "active");
  const end = (event: Event) => set(event, "idle");
  const input = (event: Event) => {
    if ((event as InputEvent).isComposing) set(event, "active");
  };
  doc.addEventListener("compositionstart", start, true);
  doc.addEventListener("compositionend", end, true);
  doc.addEventListener("input", input, true);
  doc.addEventListener("blur", end, true);
  target[TRACKER_KEY] = () => {
    doc.removeEventListener("compositionstart", start, true);
    doc.removeEventListener("compositionend", end, true);
    doc.removeEventListener("input", input, true);
    doc.removeEventListener("blur", end, true);
    delete target[TRACKER_KEY];
  };
}

// Elements for which the nested-island self-wrap warning has already been
// emitted. Guards against repeated warn spam across re-walks (e.g. SPA swaps).
const warnedNested = new WeakSet<Element>();
// Module-level captured manifest — set by the first `mountIslands` call and reused by
// `mountNewIslands()` so the client-router does not need to know the manifest directly.
// Named technical cause (W1B §12.1): the router lives in @takazudo/zfb-runtime; the
// islands manifest lives in @takazudo/zfb. Passing the manifest through the swap event
// would require widening the event API or threading manifest into router options. The
// captured-manifest pattern keeps the package boundary clean.
let capturedManifest: IslandManifest | null = null;

// Map of element → cancel-function for deferred-hydration islands
// (data-when="idle"|"visible"|"media").
// Populated in scheduleMount; consulted on `zfb:before-swap` so deferred fires do not run
// against orphan elements after a body swap. (W1B §12.5)
const pendingCancels = new Map<Element, () => void>();

/**
 * Walk the DOM and mount every `[data-zfb-island]` / `[data-zfb-island-skip-ssr]`
 * element using `manifest`.
 *
 * No-op when `document` is undefined (SSR, edge runtime). Safe to call
 * multiple times: each element is mounted at most once thanks to the
 * symbol-handle guard.
 *
 * The manifest is captured at module level so `mountNewIslands()` can re-use
 * it after an SPA body swap without needing the caller to re-supply it.
 */
export function mountIslands(manifest: IslandManifest): void {
  if (typeof document === "undefined") return;

  // Capture the manifest for post-swap re-walks via mountNewIslands().
  capturedManifest = manifest;
  installCompositionTracker(document);

  const ssrIslands = document.querySelectorAll<HTMLElement>("[data-zfb-island]");
  for (const el of Array.from(ssrIslands)) {
    const replacing = prepareMount(el);
    // Skip the empty-skeleton case left behind when the server-side
    // rewriter has not run yet (data-zfb-island="" with no component
    // name). The hydration emit step is expected to fill this in
    // before the page reaches the browser; if it didn't, we cannot
    // dispatch.
    const name = el.getAttribute("data-zfb-island");
    if (!name) continue;
    warnIfNestedIsland(el, name);
    scheduleMount(manifest, el, name, replacing ? "render" : "hydrate", { force: replacing });
  }

  const skipSsrIslands = document.querySelectorAll<HTMLElement>("[data-zfb-island-skip-ssr]");
  for (const el of Array.from(skipSsrIslands)) {
    const replacing = prepareMount(el);
    const name = el.getAttribute("data-zfb-island-skip-ssr");
    if (!name) continue;
    warnIfNestedIsland(el, name);
    scheduleMount(manifest, el, name, "render", { force: replacing });
  }
}

/**
 * Re-walk the current document body and mount any new island markers introduced
 * by an SPA body swap. Uses the manifest captured by the previous `mountIslands`
 * call — no manifest arg required.
 *
 * The caller (client-router `router.ts`) invokes this after `swap()` + `runScripts()`
 * and before dispatching `zfb:page-load`, per W1B §12.2 contract.
 *
 * No-op when called before `mountIslands` (capturedManifest is null) or when
 * `document` is undefined.
 */
export function mountNewIslands(): void {
  if (typeof document === "undefined") return;
  if (capturedManifest === null) return;

  const manifest = capturedManifest;

  const ssrIslands = document.querySelectorAll<HTMLElement>("[data-zfb-island]");
  for (const el of Array.from(ssrIslands)) {
    const replacing = prepareMount(el);
    const name = el.getAttribute("data-zfb-island");
    if (!name) continue;
    // The router marks changed persisted identity/props on the surviving node.
    const forceRemount = clearMountedForRemount(el) || replacing;
    warnIfNestedIsland(el, name);
    scheduleMount(manifest, el, name, forceRemount ? "render" : "hydrate", { force: forceRemount });
  }

  const skipSsrIslands = document.querySelectorAll<HTMLElement>("[data-zfb-island-skip-ssr]");
  for (const el of Array.from(skipSsrIslands)) {
    const replacing = prepareMount(el);
    const name = el.getAttribute("data-zfb-island-skip-ssr");
    if (!name) continue;
    warnIfNestedIsland(el, name);
    const forceRemount = clearMountedForRemount(el) || replacing;
    scheduleMount(manifest, el, name, "render", { force: forceRemount });
  }
}

/** Consume a persisted root's remount flag and dispose its previous resources. */
function clearMountedForRemount(el: Element): boolean {
  if (!el.hasAttribute(ISLAND_REMOUNT_ATTR)) return false;
  el.removeAttribute(ISLAND_REMOUNT_ATTR);
  if (rootHandle(el)) disposeIsland(el, "remount disposal");
  else el.removeAttribute(ISLAND_MOUNTED_ATTR);
  return true;
}

function prepareMount(el: Element): boolean {
  let replacing = false;
  if (rootHandle(el) && !owned.has(el)) {
    disposeIsland(el, "dev replacement disposal");
    replacing = true;
  }
  if (!rootHandle(el)) el.removeAttribute(ISLAND_MOUNTED_ATTR);
  return replacing;
}

/**
 * Cancel deferred-hydration callbacks for all islands in the old body before a
 * swap. Prevents idle / visibility callbacks from running against orphan elements
 * after `swapBodyElement` removes them from the live document. (W1B §12.5)
 *
 * Call this on `zfb:before-swap` (or equivalently, in the router's swap sequence
 * before `swap()` mutates the DOM). Fire-and-forget; safe to call if nothing is
 * pending.
 */
export function cancelPendingIslands(): void {
  for (const [el, cancel] of pendingCancels) {
    cancel();
    pendingCancels.delete(el);
  }
}

/**
 * Warn (once per element, dev-only) when an island marker element is found
 * nested inside another island marker. Self-wrapping an island — emitting a
 * `data-zfb-island` or `data-zfb-island-skip-ssr` container *inside* another
 * island component's render output — mis-hydrates because the runtime will
 * try to mount both the outer and inner islands independently. The outer
 * island's framework instance owns the inner DOM, so a second `hydrate()` /
 * `render()` call against the inner element races with the outer render and
 * produces undefined behaviour.
 *
 * The fix is to author the inner component bare (no `<Island>` in its own
 * render output) and apply the `<Island when="...">` wrapper at the call site.
 */
function warnIfNestedIsland(el: Element, componentName: string): void {
  if (typeof process === "undefined" || !process.env || process.env["NODE_ENV"] === "production") {
    return;
  }
  if (warnedNested.has(el)) return;
  const parent = el.parentElement;
  if (!parent || typeof parent.closest !== "function") return;
  const ancestor = parent.closest("[data-zfb-island],[data-zfb-island-skip-ssr]");
  if (!ancestor) return;
  warnedNested.add(el);
  // eslint-disable-next-line no-console
  console.warn(
    `[zfb] Island "${componentName}" is nested inside another island marker. ` +
      `Self-wrapping an island mis-hydrates: the outer framework instance owns ` +
      `the inner DOM, causing a conflicting mount. ` +
      `Fix: author "${componentName}" bare (remove <Island> from its own render output) ` +
      `and apply <Island when="..."> at the call site instead.`,
  );
}

function scheduleMount(
  manifest: IslandManifest,
  element: Element,
  componentName: string,
  mode: "hydrate" | "render",
  options: { force?: boolean } = {},
): void {
  if (rootHandle(element)) return;

  const entry = manifest[componentName];
  if (entry == null) {
    if (typeof process !== "undefined" && process.env && process.env["NODE_ENV"] !== "production") {
      // eslint-disable-next-line no-console
      console.warn(
        `[zfb] no island manifest entry for component "${componentName}" — ` +
          `the runtime manifest is out of sync with the rendered HTML.`,
      );
    }
    return;
  }

  fireInlineMount(element, entry, mode, options);
}

/**
 * Run the mount step for the inline-module manifest shape used by the
 * shared-bundle path. The module is already in memory (it was imported
 * into the bundle at build time), so there is no async window to
 * coordinate around — we just call `mount` / `default` directly,
 * gated by `data-when` semantics.
 */
function fireInlineMount(
  element: Element,
  mod: IslandModule,
  mode: "hydrate" | "render",
  options: { force?: boolean } = {},
): void {
  const fn = mod.mount ?? mod.default;
  if (typeof fn !== "function") {
    if (typeof process !== "undefined" && process.env && process.env["NODE_ENV"] !== "production") {
      // eslint-disable-next-line no-console
      console.warn("[zfb] inline island manifest entry did not export mount() or default()");
    }
    return;
  }

  const fire = (): void => {
    // Re-check the guard in case `fire` is invoked from a deferred
    // scheduler (rIC/rAF/visibility) after a sibling caller already
    // mounted this element.
    if (rootHandle(element)) return;
    // When the deferred fire actually runs, the cancel handle is no longer
    // needed — remove it so pendingCancels doesn't hold stale entries.
    pendingCancels.delete(element);
    // Stale-mount race guard for deferred inline mounts: skip if the element
    // was detached (e.g. body swap) while the idle/visible callback was queued.
    if (!element.isConnected) return;
    // Lazy props parse: read and parse data-props only at mount time.
    // For deferred strategies (media, visible, idle) this avoids JSON.parse
    // work at boot time for islands that may never hydrate.
    try {
      const props = readProps(element);
      const result = fn(props, element, mode);
      if (result === null) return;
      const handle: IslandRootHandle =
        result && typeof result.dispose === "function"
          ? result
          : { dispose: () => mod.unmount?.(element) };
      setRootHandle(element, handle);
      owned.add(element);
      element.setAttribute(ISLAND_MOUNTED_ATTR, "");
    } catch (error) {
      element.removeAttribute(ISLAND_MOUNTED_ATTR);
      reportIslandError(element, "mount", error);
    }
  };

  if (options.force || (mode === "render" && !element.hasAttribute("data-zfb-transport"))) {
    fire();
    return;
  }

  const when = element.getAttribute("data-when") ?? undefined;
  const { fired, cancel } = scheduleHydrateInternal(element, when, fire);
  // Track deferred-hydration cancel handle so cancelPendingIslands() can abort
  // idle / visibility callbacks before a body swap. (W1B §12.5)
  // Only register when the scheduler did NOT fire synchronously — a synchronous
  // fire means the island is already handling its mount and there is no
  // deferred callback to cancel. Registering noop after a sync fire would leave
  // a stale pendingCancels entry for an already-handled element. (#743)
  if (when && when !== "load" && !fired) {
    pendingCancels.set(element, cancel);
  }
}

/**
 * Dispose roots that the incoming body will discard. Persisted roots keep their
 * handles and DOM until the post-swap scan determines whether to recreate them.
 */
export function unmountIslands(
  root: ParentNode = document.body,
  incomingBody?: ParentNode | null,
): void {
  const selector = "[data-zfb-island],[data-zfb-island-skip-ssr]";
  // Persisted nodes are lifted by the router and retain their live resources.
  const preservedPersistIds = collectPersistIds(incomingBody);
  const elements = root.querySelectorAll<HTMLElement>(selector);
  for (const el of Array.from(elements)) {
    const persistId = el.getAttribute(PERSIST_ATTR);
    if (persistId !== null && preservedPersistIds.has(persistId)) continue;
    disposeIsland(el, "disposal");
  }
}

/**
 * Collect the `data-zfb-transition-persist` ids present in the incoming body so
 * `unmountIslands` can tell which old-body islands `swapBodyElement` will lift
 * (and therefore must be left mounted). Returns an empty set when no incoming
 * body is supplied.
 */
function collectPersistIds(incomingBody?: ParentNode | null): Set<string> {
  const ids = new Set<string>();
  if (!incomingBody) return ids;
  for (const el of incomingBody.querySelectorAll(`[${PERSIST_ATTR}]`)) {
    const id = el.getAttribute(PERSIST_ATTR);
    if (id !== null) ids.add(id);
  }
  return ids;
}

function readProps(element: Element): Record<string, unknown> {
  const raw = element.getAttribute("data-props");
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as unknown;
    // Reject arrays explicitly: `typeof [] === "object"` is true but
    // an array is not a valid props bag, and passing it through would
    // mean the component receives index-keyed values where it
    // expected a record. Fall through to the empty-object default
    // instead of forwarding a malformed shape.
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
  } catch {
    // fall through
  }
  return {};
}

/**
 * Test-only seam. Returns whether the given element has an entry in the
 * module-private `pendingCancels` Map. Used to assert that a synchronous
 * scheduler fire does not leave a stale entry behind. (#743)
 */
export function __hasPendingCancelForTests(element: Element): boolean {
  return pendingCancels.has(element);
}
