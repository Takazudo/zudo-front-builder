// =============================================================================
// zfb islands hydration runtime
// -----------------------------------------------------------------------------
// Size budget: ~3 KB before gzip (the issue body's "naive ~3 KB" target).
// React's bundled hydration glue can balloon this to ~30 KB; that growth lives
// in the framework code itself, not here. The cost we control is THIS file.
// To hold the budget:
//
//   * No try/catch wrapping that swallows errors — let them surface.
//   * No legacy-browser polyfills (no IE, no SystemJS).
//   * No framework-specific branches: all framework calls go through the
//     adapter shim's hydrateIsland(Component, props, element) export.
//   * No third-party deps. Plain DOM + dynamic import().
//
// The runtime is a single module loaded via:
//   <script type="module" src="…runtime.js" data-zfb-bundle="…/islands-{hash}.js"></script>
// It walks every [data-zfb-island] element, decodes data-props, dispatches on
// data-when (load|idle|visible|media), and calls the adapter shim's hydrateIsland.
//
// The data-when dispatch is delegated to scheduleHydrate from "@takazudo/zfb/runtime"
// so the wrapper (Sub 4) and the runtime (Sub 3) share one source of truth
// for visible|idle|load|media semantics.
// =============================================================================

import { scheduleHydrate } from "@takazudo/zfb/runtime";

/** Public surface so the bundle entry can re-export this if needed. */
export interface HydrateIslandFn {
  (Component: unknown, props: unknown, element: Element): void;
}

/**
 * Shape of the islands bundle that the data-zfb-bundle URL resolves to.
 *
 * Components are exposed as named exports; `hydrateIsland` is the
 * framework-specific shim contributed by zfb-render's adapter (Preact uses
 * `h`+`hydrate`; React uses `hydrateRoot` from `react-dom/client`).
 *
 * The bundler (Sub 2) is responsible for arranging both pieces in the same
 * module so this single dynamic import resolves everything.
 */
type IslandsBundle = {
  hydrateIsland: HydrateIslandFn;
  // Other named exports are the islands themselves, keyed by component name.
  [componentName: string]: unknown;
};

/** Recognised values for data-when. Absent ⇒ "load". */
const WHEN_VALUES = ["load", "idle", "visible", "media"] as const;
type When = (typeof WHEN_VALUES)[number];

/** Pull the islands-bundle URL off the runtime's own <script> tag. */
function readBundleUrl(): string {
  // currentScript only works during initial parse, so we also accept any
  // script tag carrying the data-zfb-bundle attribute as a fallback for
  // late-loaded module scripts.
  const fromCurrent = document.currentScript as HTMLScriptElement | null;
  const url =
    fromCurrent?.dataset?.zfbBundle ??
    document.querySelector<HTMLScriptElement>("script[data-zfb-bundle]")?.dataset.zfbBundle;
  if (!url) {
    throw new Error("zfb-islands: no <script data-zfb-bundle> found; cannot locate islands bundle");
  }
  return url;
}

/** Coerce a data-when string into the supported set. */
function readWhen(el: Element): When {
  const raw = (el as HTMLElement).dataset.when;
  if (raw == null || raw === "") return "load";
  if ((WHEN_VALUES as readonly string[]).includes(raw)) return raw as When;
  // Unknown value: fall back to immediate. Surfacing a warning beats
  // pretending we understood it; the build pipeline should have caught
  // misspellings already.
  // eslint-disable-next-line no-console
  console.warn(`zfb-islands: unknown data-when="${raw}" on island; defaulting to "load"`);
  return "load";
}

/** Parse data-props as JSON. Empty/missing ⇒ {}. */
function readProps(el: Element): unknown {
  const raw = (el as HTMLElement).dataset.props;
  if (raw == null || raw === "") return {};
  return JSON.parse(raw);
}

/** Read the component name. Returns null if missing/empty. */
function readComponentName(el: Element): string | null {
  const name = (el as HTMLElement).dataset.zfbIsland;
  if (name == null || name === "") return null;
  return name;
}

/**
 * Hydrate one island. Looks up Component from the loaded bundle by name,
 * decodes props, and delegates to the adapter's hydrateIsland.
 */
function hydrateOne(bundle: IslandsBundle, el: Element): void {
  const name = readComponentName(el);
  if (name === null) {
    // No name ⇒ no work to do; surface a clear error so the build pipeline
    // can catch missing data-zfb-island wiring early.
    throw new Error("zfb-islands: element has no data-zfb-island component name");
  }
  const Component = bundle[name];
  if (Component === undefined) {
    throw new Error(`zfb-islands: islands bundle has no export "${name}"`);
  }
  const props = readProps(el);
  bundle.hydrateIsland(Component, props, el);
}

/**
 * Schedule hydrateOne according to the element's data-when hint.
 *
 *   load    → run synchronously (immediate)
 *   idle    → requestIdleCallback (fallback: setTimeout 0)
 *   visible → IntersectionObserver
 *   media   → matchMedia (query read off the element's data-media attribute)
 *
 * The dispatch table lives in scheduleHydrate from "zfb/runtime" so this
 * runtime and the <Island> wrapper share the same When semantics. We just
 * adapt the (target, when, fire) signature here.
 */
function schedule(when: When, el: Element, run: (el: Element) => void): void {
  scheduleHydrate(el, when, () => run(el));
}

/**
 * Entry point. Exported so callers / tests can drive the runtime
 * deterministically. The default behaviour (auto-start on module load)
 * happens at the bottom of this file.
 */
export async function hydrateAll(opts?: {
  /** Override the bundle URL. Defaults to reading data-zfb-bundle. */
  bundleUrl?: string;
  /** Override which root to scan. Defaults to document. */
  root?: ParentNode;
  /**
   * Override the bundle import. Lets tests inject a fake without going
   * through dynamic import resolution. When unset, dynamic import() is used.
   */
  importBundle?: (url: string) => Promise<IslandsBundle>;
}): Promise<void> {
  const root = opts?.root ?? document;
  const elements = root.querySelectorAll<HTMLElement>("[data-zfb-island]");
  if (elements.length === 0) return;

  const url = opts?.bundleUrl ?? readBundleUrl();
  const importer =
    opts?.importBundle ?? ((u: string) => import(/* @vite-ignore */ u) as Promise<IslandsBundle>);
  const bundle = await importer(url);
  if (typeof bundle.hydrateIsland !== "function") {
    throw new Error("zfb-islands: bundle has no hydrateIsland export (adapter shim missing)");
  }

  for (const el of elements) {
    const when = readWhen(el);
    schedule(when, el, (target) => hydrateOne(bundle, target));
  }
}

// Auto-start. Skipped during unit tests by guarding on a marker attribute the
// test harness sets on the DOM before importing the module — but the simpler
// guard is "no [data-zfb-island] in the document" which the test environment
// can arrange. Tests that exercise hydrateAll call it directly.
if (typeof document !== "undefined" && document.querySelector("[data-zfb-island]")) {
  // Fire and forget; errors propagate to window.onerror.
  void hydrateAll();
}
