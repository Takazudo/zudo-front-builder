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
// The per-island bundles each export a `mount(props, element, mode)`
// function (see zfb_islands::render_island_entry_source). The
// framework-specific glue lives inside that bundle, so this runtime is
// framework-agnostic.
//
// ## Module-level singleton
//
// Dynamic imports of the same URL are cached by the JS runtime, so
// "switching pages" (in an SPA shell) reuses the loaded bundle for free.
// We keep an extra in-memory dedup map keyed by element so an island is
// never mounted twice (e.g. on hot-reload / repeat-mount scenarios).
// ---------------------------------------------------------------------------

/**
 * The shape of the default export each per-island bundle ships.
 *
 * `mode === "hydrate"` is used for SSR'd islands, `"render"` for
 * SSR-skip islands.
 */
type IslandMount = (
  props: Record<string, unknown>,
  element: Element,
  mode: "hydrate" | "render",
) => void;

type IslandUnmount = (element: Element) => void;

interface IslandModule {
  mount?: IslandMount;
  default?: IslandMount;
  unmount?: IslandUnmount;
}

/**
 * Map of `componentName → island descriptor` baked into the runtime entry.
 *
 * Two descriptor shapes are accepted so the same `mountIslands` runtime
 * handles both bundling strategies the build emits:
 *
 *   1. `string` — a per-island bundle URL. The runtime fetches it via
 *      dynamic `import()` and reads `mount` / `default` off the loaded
 *      module. Used by the per-island bundling path
 *      (`bundle_per_island` / `render_runtime_entry_source`).
 *
 *   2. `IslandModule` — an inline module-shaped object whose `mount` (or
 *      `default`) is called directly. Used by the shared-bundle path
 *      (`render_shared_bundle_entry_source`): every island's source code
 *      is already in the same bundle, so the synthesised entry can hand
 *      the runtime the constructed mount functions inline without a
 *      second HTTP fetch. This preserves the one-request shared-bundle
 *      contract while giving up nothing on hydration semantics
 *      (zudolab/zudo-doc#1355 wave 6).
 */
export type IslandManifestValue = string | IslandModule;
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
 * Public DOM signal written after an island's mount function returns.
 *
 * State table (the marker is observational only and is never a mount guard):
 *
 * - initial: absent; `mountIslands` / `mountNewIslands` strip a marker that is
 *   stale relative to this module instance's `mounted` map before scheduling.
 * - deferred idle / visible / media: absent while the scheduler is waiting.
 * - importing: absent while the URL module is in `pending`.
 * - mounted via URL: `scheduleMount`'s URL success handler writes it only after
 *   `fn(propsForMount, element, mode)` returns, alongside the `mounted` entry.
 * - mounted via inline module: `fireInlineMount` writes it only after
 *   `fn(props, element, mode)` returns, alongside the `mounted` entry.
 * - missing manifest entry: absent; `scheduleMount` returns without writing.
 * - no `mount` export: absent; both manifest paths return without writing.
 * - synchronous mount throw: absent; the `mounted` entry is not written, so a
 *   later walk can retry the element.
 * - rejected import: absent; the URL rejection handler clears `pending` and
 *   any defensive `mounted` entry.
 * - detached during import: absent; the URL success handler clears `pending`
 *   and returns before calling mount.
 * - unmounted (discarded): `unmountIslands` clears the marker and `mounted`
 *   entry in `finally`, even when the unmount thunk throws.
 * - unmounted (persisted-lifted): retained together with the `mounted` entry;
 *   `unmountIslands` skips elements whose persist id exists in the incoming body.
 * - props-changed remount: `clearMountedForRemount` clears the marker and map
 *   entry in `finally`, then the forced mount writes it again after mount returns.
 * - dev hot-swap over a marked DOM: a fresh module's `mountIslands` strips the
 *   stale marker before scheduling, then writes it after its own mount returns.
 */
export const ISLAND_MOUNTED_ATTR = "data-zfb-island-mounted";

// WeakMap<Element, unmount thunk> — replaces the old WeakSet.
// Value is a per-element function that calls the bundle's unmount(element)
// (or a noop if the bundle does not expose one). Used by unmountIslands()
// to fire framework lifecycle cleanups before a body swap.
const mounted = new WeakMap<Element, () => void>();
// Elements for which the nested-island self-wrap warning has already been
// emitted. Guards against repeated warn spam across re-walks (e.g. SPA swaps).
const warnedNested = new WeakSet<Element>();
// Elements with an in-flight dynamic import that has not yet resolved.
// Two concurrent `mountIslands` invocations (or two `scheduleMount`
// calls hitting the same element through different code paths) could
// otherwise both pass the `mounted` guard and both spawn an
// `importIsland(url)` -> `fn()` chain, double-mounting the component.
// Adding the element to `pending` synchronously, before the import is
// fired, closes that window; the entry is removed in both the success
// (after `mounted.set`) and failure branches.
const pending = new WeakSet<Element>();

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
 * `mounted` WeakSet guard.
 *
 * The manifest is captured at module level so `mountNewIslands()` can re-use
 * it after an SPA body swap without needing the caller to re-supply it.
 */
export function mountIslands(manifest: IslandManifest): void {
  if (typeof document === "undefined") return;

  // Capture the manifest for post-swap re-walks via mountNewIslands().
  capturedManifest = manifest;

  const ssrIslands = document.querySelectorAll<HTMLElement>("[data-zfb-island]");
  for (const el of Array.from(ssrIslands)) {
    stripStaleMountedMarker(el);
    // Skip the empty-skeleton case left behind when the server-side
    // rewriter has not run yet (data-zfb-island="" with no component
    // name). The hydration emit step is expected to fill this in
    // before the page reaches the browser; if it didn't, we cannot
    // dispatch.
    const name = el.getAttribute("data-zfb-island");
    if (!name) continue;
    warnIfNestedIsland(el, name);
    scheduleMount(manifest, el, name, "hydrate");
  }

  const skipSsrIslands = document.querySelectorAll<HTMLElement>("[data-zfb-island-skip-ssr]");
  for (const el of Array.from(skipSsrIslands)) {
    stripStaleMountedMarker(el);
    const name = el.getAttribute("data-zfb-island-skip-ssr");
    if (!name) continue;
    warnIfNestedIsland(el, name);
    scheduleMount(manifest, el, name, "render");
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
    stripStaleMountedMarker(el);
    const name = el.getAttribute("data-zfb-island");
    if (!name) continue;
    // A persisted island whose props changed across the body swap is flagged
    // for remount by swap-functions.swapBodyElement. Clear its surviving mounted
    // entry BEFORE scheduleMount's already-mounted guard so it re-mounts fresh
    // with the refreshed data-props. No-op for every other element.
    const forceRemount = clearMountedForRemount(el);
    warnIfNestedIsland(el, name);
    scheduleMount(manifest, el, name, "hydrate", { force: forceRemount });
  }

  const skipSsrIslands = document.querySelectorAll<HTMLElement>("[data-zfb-island-skip-ssr]");
  for (const el of Array.from(skipSsrIslands)) {
    stripStaleMountedMarker(el);
    const name = el.getAttribute("data-zfb-island-skip-ssr");
    if (!name) continue;
    warnIfNestedIsland(el, name);
    scheduleMount(manifest, el, name, "render");
  }
}

/**
 * Consume the cross-package "needs-remount" signal for the persist-props hybrid
 * path (port-spec §12.3.1 hybrid case / §12.3.2). When a persisted island's
 * props differ from the incoming markup, `swapBodyElement` refreshes the
 * surviving element's `data-props` and marks it with `ISLAND_REMOUNT_ATTR`.
 * That attribute is the ONLY channel that crosses the zfb-runtime → zfb package
 * boundary — the `mounted` map is module-private to this file, so a shared
 * in-memory "needs-remount" queue between the two packages is impossible; the
 * live DOM node carrying the flag IS the queue.
 *
 * On a flagged mounted element: fire the old instance's unmount thunk (so its
 * useEffect/framework cleanups run against the still-connected node), drop the
 * `mounted` entry so `scheduleMount`'s guard no longer short-circuits, strip the
 * flag, and ask the caller to force the replacement mount through immediately
 * instead of re-entering any deferred scheduler. This keeps a deferred persisted
 * island from blanking while it waits for idle/visible/media to fire again.
 *
 * On a flagged element whose URL import is still pending, leave the flag in
 * place. The already-running import's success handler consumes it after the
 * module resolves and re-reads `data-props` at that point, so a props refresh
 * that happened during the import wins without starting a duplicate import.
 *
 * A no-op for elements without the flag (the common case: fresh markers and
 * props-unchanged persisted islands).
 *
 * Scope: only the `[data-zfb-island]` (hydrated) loop calls this, mirroring the
 * writer side — swapBodyElement sets the flag only for `newTarget.matches(
 * "[data-zfb-island]")`, never for skip-ssr islands.
 */
function clearMountedForRemount(el: Element): boolean {
  if (!el.hasAttribute(ISLAND_REMOUNT_ATTR)) return false;
  if (pending.has(el)) return false;

  const thunk = mounted.get(el);
  if (thunk) {
    try {
      thunk();
    } finally {
      mounted.delete(el);
      el.removeAttribute(ISLAND_MOUNTED_ATTR);
      el.removeAttribute(ISLAND_REMOUNT_ATTR);
    }
    return true;
  }
  el.removeAttribute(ISLAND_MOUNTED_ATTR);
  el.removeAttribute(ISLAND_REMOUNT_ATTR);
  return false;
}

function stripStaleMountedMarker(el: Element): void {
  if (!mounted.has(el)) el.removeAttribute(ISLAND_MOUNTED_ATTR);
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
  // Skip elements already mounted OR currently importing — the latter
  // prevents two concurrent `mountIslands` calls from each firing a
  // separate dynamic import for the same element.
  if (mounted.has(element) || pending.has(element)) return;

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

  const when = element.getAttribute("data-when") ?? undefined;

  // Two manifest shapes:
  //
  //   - `string` (per-island bundle URL): fetch via dynamic `import()`
  //     and call `mount` / `default` on the resolved module.
  //   - `IslandModule` (inline descriptor): the shared-bundle path has
  //     already imported every island's source into the same bundle and
  //     constructed a mount function for it. Skip the dynamic import
  //     and call the supplied function directly.
  if (typeof entry !== "string") {
    fireInlineMount(element, entry, mode, options);
    return;
  }

  const url: string = entry;

  const fire = (): void => {
    // Re-check both guards in case `fire` is invoked from a deferred
    // scheduler (rIC/rAF/visibility) after a sibling caller already
    // mounted or started importing for this element.
    if (mounted.has(element) || pending.has(element)) return;

    // When the deferred fire actually runs, the cancel handle is no longer
    // needed — remove it so pendingCancels doesn't hold stale entries.
    pendingCancels.delete(element);

    // Lazy props parse: read and parse data-props only now that we know we
    // are actually going to mount this island. For deferred strategies
    // (media, visible, idle) this avoids JSON.parse work at boot time for
    // islands that may never hydrate (e.g. media query never matches).
    const props = readProps(element);

    // Mark as pending BEFORE firing the import so any concurrent
    // `mountIslands` invocation that arrives during the await window
    // is short-circuited by `scheduleMount`'s guard.
    pending.add(element);

    // Dynamic-import is cached by the JS runtime, so repeat hits for
    // the same URL share the resolved module — module-level
    // singletons are fine.
    //
    // We move the element from `pending` to `mounted` only on the
    // success path so a failed import (e.g. transient network blip
    // in dev) doesn't permanently block a retry of the same element.
    let started: Promise<IslandModule>;
    try {
      started = importIsland(url);
    } catch (err) {
      // Some implementations of dynamic-import wrappers can throw
      // synchronously (e.g. URL parsing errors). Treat the same as
      // an async rejection.
      pending.delete(element);
      // eslint-disable-next-line no-console
      console.error(`[zfb] failed to start dynamic import for ${url}`, err);
      return;
    }
    started.then(
      (mod) => {
        const fn = mod.mount ?? mod.default;
        if (typeof fn !== "function") {
          pending.delete(element);
          if (
            typeof process !== "undefined" &&
            process.env &&
            process.env["NODE_ENV"] !== "production"
          ) {
            // eslint-disable-next-line no-console
            console.warn(`[zfb] island bundle at ${url} did not export mount() or default()`);
          }
          return;
        }
        // Stale-mount race guard: if the element was detached while the
        // dynamic import was in-flight (e.g. a body swap happened), skip
        // mounting — the element is no longer in the live document and
        // its useEffect listeners would never receive a cleanup call.
        if (!element.isConnected) {
          pending.delete(element);
          return;
        }
        const shouldRefreshProps = element.hasAttribute(ISLAND_REMOUNT_ATTR);
        const propsForMount = shouldRefreshProps ? readProps(element) : props;
        if (shouldRefreshProps) element.removeAttribute(ISLAND_REMOUNT_ATTR);
        const unmountThunk = mod.unmount
          ? () => mod.unmount!(element)
          : () => {
              // noop — bundle does not expose unmount
            };
        try {
          fn(propsForMount, element, mode);
          mounted.set(element, unmountThunk);
          element.setAttribute(ISLAND_MOUNTED_ATTR, "");
        } finally {
          pending.delete(element);
        }
      },
      (err: unknown) => {
        // Surface the error in dev so the user notices, then clear
        // both guards so a later retry (e.g. another scheduleHydrate
        // fire) can attempt the import again.
        pending.delete(element);
        mounted.delete(element);
        // eslint-disable-next-line no-console
        console.error(`[zfb] failed to load island bundle ${url}`, err);
      },
    );
  };

  if (mode === "render") {
    // SSR-skip islands ignore data-when: there is nothing to defer
    // hydration of, just an empty container we paint into. Mount
    // immediately so the user sees output.
    fire();
    return;
  }

  if (options.force) {
    fire();
    return;
  }

  const { fired, cancel } = scheduleHydrateInternal(element, when, fire);
  // Track deferred-hydration cancel handle so cancelPendingIslands() can abort
  // idle / visibility callbacks before a body swap. (W1B §12.5)
  // Only register when the scheduler did NOT fire synchronously — a synchronous
  // fire means the island is already handling its import and there is no
  // deferred callback to cancel. Registering noop after a sync fire would leave
  // a stale pendingCancels entry for an already-handled element. (#743)
  if (when && when !== "load" && !fired) {
    pendingCancels.set(element, cancel);
  }
}

/**
 * Run the mount step for the inline-module manifest shape used by the
 * shared-bundle path. The module is already in memory (it was imported
 * into the bundle at build time), so there is no async window to
 * coordinate around — we just call `mount` / `default` directly,
 * gated by the same `data-when` semantics as the URL path.
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
    if (mounted.has(element)) return;
    // When the deferred fire actually runs, the cancel handle is no longer
    // needed — remove it so pendingCancels doesn't hold stale entries.
    pendingCancels.delete(element);
    // Stale-mount race guard for deferred inline mounts: skip if the element
    // was detached (e.g. body swap) while the idle/visible callback was queued.
    if (!element.isConnected) return;
    // Lazy props parse: read and parse data-props only at mount time.
    // For deferred strategies (media, visible, idle) this avoids JSON.parse
    // work at boot time for islands that may never hydrate.
    const props = readProps(element);
    const unmountThunk = mod.unmount
      ? () => mod.unmount!(element)
      : () => {
          // noop — inline module does not expose unmount
        };
    fn(props, element, mode);
    mounted.set(element, unmountThunk);
    element.setAttribute(ISLAND_MOUNTED_ATTR, "");
  };

  if (mode === "render") {
    fire();
    return;
  }

  if (options.force) {
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
 * Unmount the mounted islands within `root` (default: `document.body`) that will
 * NOT survive the body swap.
 *
 * Walks `root` for `[data-zfb-island]` and `[data-zfb-island-skip-ssr]` elements,
 * looks up each element's unmount thunk in the `mounted` WeakMap, calls it (which
 * triggers `render(null, element)` for Preact or `root.unmount()` for React), and
 * removes the entry from the map so `mountNewIslands()` can re-mount later.
 *
 * Call this before `swapBodyElement(...)` so the OLD body's islands receive proper
 * framework lifecycle cleanup (useEffect teardowns, etc.) before being discarded.
 *
 * When `incomingBody` is supplied (the client-router passes the parsed incoming
 * document body), any island whose `data-zfb-transition-persist` id matches a
 * marker in that body is DELIBERATELY SKIPPED: swapBodyElement will physically
 * lift the node into the new body, so its component instance and internal state
 * must survive — unmounting it here would empty the container before the lift and
 * defeat the persist contract (issue #1389). Omit `incomingBody` (or pass null)
 * to unmount everything, the pre-#1389 behavior.
 *
 * No-op for elements not in the `mounted` map (e.g. never-mounted or already cleaned up).
 */
export function unmountIslands(
  root: ParentNode = document.body,
  incomingBody?: ParentNode | null,
): void {
  const selector = "[data-zfb-island],[data-zfb-island-skip-ssr]";
  // Persist ids that `swapBodyElement` will physically LIFT from the old body
  // into the incoming body — an old marker survives iff the incoming body has a
  // marker with the same `data-zfb-transition-persist` id. Those DOM nodes are
  // moved, not discarded, so their component instance and internal state MUST
  // survive the swap: skip their framework unmount here or the persist contract
  // preserves nothing (port-spec §12.3.1 case (a) / issue #1389). A persisted
  // island whose props changed is skipped here too — its refreshed remount runs
  // later in mountNewIslands via the `data-zfb-island-remount` flag (see
  // `clearMountedForRemount`) swapBodyElement sets. With no incoming body (a
  // call outside a swap) nothing is preserved, so the walk is byte-identical to
  // the pre-#1389 behavior.
  const preservedPersistIds = collectPersistIds(incomingBody);
  const elements = root.querySelectorAll<HTMLElement>(selector);
  for (const el of Array.from(elements)) {
    const persistId = el.getAttribute(PERSIST_ATTR);
    if (persistId !== null && preservedPersistIds.has(persistId)) continue;
    const thunk = mounted.get(el);
    try {
      thunk?.();
    } finally {
      mounted.delete(el);
      el.removeAttribute(ISLAND_MOUNTED_ATTR);
    }
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
 * Indirection so tests can stub the dynamic import without intercepting
 * the global `import()`. In production this is a thin wrapper over
 * native `import(url)`.
 */
let importImpl: (url: string) => Promise<IslandModule> = (url) =>
  // Modern bundlers (esbuild, Vite, Rollup, webpack) preserve a plain
  // `import(<dynamic>)` call when the argument isn't a static literal,
  // so we no longer need the `new Function(...)` indirection — which
  // also failed under strict CSPs that disallow `unsafe-eval`.
  import(/* @vite-ignore */ /* webpackIgnore: true */ url) as Promise<IslandModule>;

function importIsland(url: string): Promise<IslandModule> {
  return importImpl(url);
}

/**
 * Test-only seam. Replace the module dynamic-import with a fake.
 * Returns the previous implementation so tests can restore it.
 */
export function __setIslandImporterForTests(
  impl: (url: string) => Promise<IslandModule>,
): (url: string) => Promise<IslandModule> {
  const prev = importImpl;
  importImpl = impl;
  return prev;
}

/**
 * Test-only seam. Returns whether the given element has an entry in the
 * module-private `pendingCancels` Map. Used to assert that a synchronous
 * scheduler fire does not leave a stale entry behind. (#743)
 */
export function __hasPendingCancelForTests(element: Element): boolean {
  return pendingCancels.has(element);
}
