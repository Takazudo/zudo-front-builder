// `zfb/content` — minimal v0 content collection loader.
//
// Reads `*.md` files from a content collection directory, parses YAML
// frontmatter, and returns typed entries. This is a deliberately small
// stub so the bundled basic-blog template can call `getCollection("blog")` today;
// the production path lives in `crates/zfb-content` and will replace this
// once the JS-runtime decision (ADR-001) lands and the renderer wires the
// Rust pipeline back through to user code.
//
// Scope (v0):
// - YAML-ish frontmatter only: `key: value`, plus `key:\n  - item` arrays.
//   Quoted strings are unwrapped. ISO dates stay as strings.
// - Body is the post content **after** the closing `---`, returned as raw
//   text. This is intentionally NOT pre-rendered HTML: the markdown
//   pipeline lives in the Rust crate and the JS stub does not duplicate
//   it.
// - Collection root is resolved from
//   `process.env.ZFB_CONTENT_ROOT` (set by the dev/build pipeline), or
//   `<cwd>/content` as a fallback for unit tests and direct invocation.
//
// TODO(zfb-content): swap this stub for the runtime-provided implementation
// once the content engine ships end-to-end.

// `node:fs` and `node:path` are intentionally NOT imported statically at
// the top level.
//
// Why: this module is reachable via the package root (`@takazudo/zfb`) — the
// barrel re-exports `defaultComponents` / `ContentH2` / etc. from
// `./content.js`. The islands per-island bundler (`crates/zfb-islands`) walks
// `import * as Mod from "@takazudo/zfb"` and esbuild's static tree-shaker
// cannot prune a module behind a wildcard barrel access, so the WHOLE
// content.ts module ends up in the browser-side island bundle. Top-level
// `node:fs` / `node:path` imports would then fail the bundle with
// `Could not resolve "node:fs"`. Loading them indirectly through a
// runtime-constructed `createRequire` keeps the Node-runtime fs path
// working while letting the islands bundler emit browser-safe output.
// (Discovered while investigating zudolab/zudo-doc#1355 Wave 3 — see also
// upstream PR #134 / #130 Gap A.) Defense-in-depth: the islands esbuild
// invocation also passes `--platform=browser --external:node:*` so any
// stray `node:*` import that does end up in a browser bundle is
// externalized rather than failing the build.
//
// `getCollection` is synchronous per ADR-004, so the node modules are
// loaded synchronously on first fs-path use. Type-only imports below stay
// at the top because TypeScript erases them at compile time — they leave
// no runtime traces for esbuild to chase.
import { jsx } from "react/jsx-runtime";

import type * as NodeFs from "node:fs";
import type * as NodePath from "node:path";

import { parseFrontmatter } from "./frontmatter.js";
import type { ParsedFrontmatter } from "./frontmatter.js";
import type { VNode } from "./jsx-types.js";

// Re-export the parser surface so existing `zfb/content` consumers that
// import `parseFrontmatter` / `ParsedFrontmatter` from the content
// subpath keep working. The implementation now lives in `./frontmatter.ts`
// (BCI-3 fs-free subpath) — this re-export is the bridge for callers
// that have not migrated yet.
export { parseFrontmatter };
export type { ParsedFrontmatter };

// ---------------------------------------------------------------------------
// In-memory ContentSnapshot bridge (consumed by `@takazudo/zfb-runtime`).
//
// At build time, the Rust pipeline produces a `ContentSnapshot` (see
// `crates/zfb-content/src/content_bridge.rs`) and embeds it into the
// Worker bundle. On Worker boot, `createPageRouter` calls
// `setContentSnapshot(snapshot)` (below) before serving the first
// request. From that point on, `getCollection(name)` resolves from the
// embedded snapshot rather than the Node `fs` API — required because the
// workerd / Cloudflare Workers runtime has no filesystem.
//
// The fs path remains the source of truth in two contexts:
//   1. unit tests for this module (no snapshot installed → fs path),
//   2. dev-preview / direct-Node invocations of `getCollection` outside
//      the Worker bundle (kept as v0 fallback so older callers still work).
//
// Keep [`SnapshotEntry`] / [`Snapshot`] aligned with the Rust struct
// (`EntrySnapshot` / `ContentSnapshot`) and the runtime-package mirror
// (`@takazudo/zfb-runtime/snapshot`). Field names are snake_case to
// match the JSON serialization (`module_specifier`, `rel_path`).
// ---------------------------------------------------------------------------

/**
 * One entry in an embedded content snapshot. Mirrors
 * `crates/zfb-content/src/content_bridge.rs::EntrySnapshot`. Re-exported
 * by `@takazudo/zfb-runtime/snapshot` for the runtime-side bundle. See
 * that module for field-by-field documentation.
 */
export interface SnapshotEntry {
  readonly slug: string;
  readonly frontmatter: unknown;
  readonly body: string;
  readonly module_specifier: string;
  readonly rel_path: string;
}

/**
 * Point-in-time snapshot of every configured collection. Mirrors
 * `crates/zfb-content/src/content_bridge.rs::ContentSnapshot`.
 */
export interface Snapshot {
  readonly collections: Readonly<Record<string, readonly SnapshotEntry[]>>;
}

/**
 * Where the installed [`Snapshot`] lives.
 *
 * The state hangs off `globalThis.__zfb.contentSnapshot`, NOT a
 * module-level `let`. This matters because under the production worker
 * bundle the consumer's pnpm-strict `node_modules` layout exposes
 * two physical paths to `@takazudo/zfb`:
 *
 *   - top-level `node_modules/@takazudo/zfb` (imported by user pages), AND
 *   - nested `node_modules/.pnpm/@takazudo+zfb-runtime@.../node_modules/
 *     @takazudo/zfb` (imported by `@takazudo/zfb-runtime` itself).
 *
 * The bundler passes `esbuild --preserve-symlinks` whenever a custom
 * `node_modules_dir` is configured (see `crates/zfb-build/src/bundler.rs`
 * around `--external:node:*`), so esbuild treats those two symlink
 * targets as distinct sources and inlines `content.js` TWICE — yielding
 * two module instances of `zfb/content` in the final worker bundle.
 *
 * If `installedSnapshot` were a per-module `let`, `createPageRouter`
 * would install the snapshot on the runtime's copy and `getCollection`
 * (called from a user `paths()` export) would read from the user
 * page's copy — see `undefined`, and fall through to the `node:fs`
 * branch, which then throws because `node:*` is externalized in the
 * worker bundle. This is the regression #442 / #449 surfaced.
 *
 * Routing the slot through `globalThis` makes the snapshot bridge
 * symmetric with the existing `globalThis.__zfb.content` MDX-component
 * bridge (set by the build pipeline at `crates/zfb-build/src/bundler.rs`,
 * read by `Content` below): both pieces of cross-module state share
 * one well-known global, so any number of `zfb/content` module
 * instances in the same JS realm see the same value.
 *
 * Tracked under #449 (production fix for #442); the test-fixture
 * counterpart was #413.
 */
type SnapshotBridgeNamespace = {
  contentSnapshot?: Snapshot | undefined;
};

type SnapshotBridgeGlobal = typeof globalThis & {
  __zfb?: SnapshotBridgeNamespace;
};

/**
 * Register a [`Snapshot`] so [`getCollection`] resolves from memory.
 *
 * Pass `undefined` to clear (used by tests that need to restore the v0
 * filesystem path between runs). Idempotent: the latest call wins.
 *
 * Stored on `globalThis.__zfb.contentSnapshot` rather than a
 * module-level `let` so a worker bundle that ends up with two
 * `zfb/content` module instances still sees a single shared snapshot —
 * see the [`SnapshotBridgeNamespace`] doc above for the full
 * pnpm-symlink rationale.
 */
export function setContentSnapshot(snapshot: Snapshot | undefined): void {
  const g = globalThis as SnapshotBridgeGlobal;
  const ns = (g.__zfb ?? {}) as SnapshotBridgeNamespace;
  ns.contentSnapshot = snapshot;
  g.__zfb = ns;
}

/**
 * Read the currently-installed [`Snapshot`], or `undefined` if none is
 * registered. Exposed mostly for tests; production callers should not
 * need to introspect the bridge state.
 *
 * Reads from `globalThis.__zfb.contentSnapshot`; see
 * [`setContentSnapshot`] for why the slot lives on `globalThis`.
 */
export function getContentSnapshot(): Snapshot | undefined {
  return (globalThis as SnapshotBridgeGlobal).__zfb?.contentSnapshot;
}

/**
 * Props accepted by an entry's [`CollectionEntry.Content`] component.
 *
 * `components` mirrors Astro's `<Content components={...}>` contract:
 * a flat record of element-name → override component (e.g. `{ h1: MyH1 }`).
 * The default-components convention ships from `zfb`'s root export
 * (`defaultComponents`, lands in Sub 6) and users compose with their own
 * via `{ ...defaultComponents, ...mine }`.
 */
export interface ContentProps {
  /** Element-name → override component map. Optional. */
  components?: Record<string, unknown>;
}

/**
 * Public JSX-element shape returned by [`CollectionEntry.Content`].
 *
 * Matches the structural shape that both Preact's and React's `jsx-runtime`
 * accept on either side of the boundary, mirroring the Island wrapper's
 * approach. Consumers should treat this as opaque — its only contract is
 * "renderable JSX value".
 *
 * Aliased as `JSX.Element` in the field signature: the JS runtime is
 * type-erased and the actual VNode shape is supplied by the framework
 * adapter at evaluation time.
 */
export type ContentElement = {
  readonly type: string | ((...args: unknown[]) => unknown);
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

/**
 * Bridge contract published by the Rust-side `zfb-render` `Renderer` before
 * evaluating each page module. Cross-referenced from the Rust side in
 * `crates/zfb-render/src/loader.rs` so the two halves stay in sync — see
 * `packages/zfb/CONTRIBUTING.md` for the full contract narrative.
 *
 * The renderer installs `globalThis.__zfb.content.get(specifier)` keyed on
 * the entry's `module_specifier` (Sub 4 convention: `mdx://<collection>/<slug>#<hash>`,
 * collapsed to `mdx://<collection>/<slug>` from the JS stub side which has
 * no hash to compute). When `get` returns `undefined` (or the bridge as a
 * whole is absent — typical of unit tests, dev sandboxes, and any
 * non-renderer evaluation context), `Content` renders a clearly-marked
 * `<pre data-zfb-content-fallback>` fallback so the visual distinction is
 * obvious even in unstyled environments.
 */
type ContentBridge = {
  get(specifier: string): ((props: ContentProps) => unknown) | undefined;
};

type ZfbBridgeNamespace = {
  content?: ContentBridge;
};

type BridgeGlobal = typeof globalThis & {
  __zfb?: ZfbBridgeNamespace;
};

/**
 * Generic shape returned for one entry in a content collection. The `data`
 * field carries parsed frontmatter, typed by the caller via the generic
 * parameter.
 */
export type CollectionEntry<T = Record<string, unknown>> = {
  /** Filename without `.md` extension. Stable across runs. */
  slug: string;
  /** Parsed frontmatter. */
  data: T;
  /** Raw markdown body (frontmatter stripped). */
  body: string;
  /**
   * Stable module specifier used as the bridge lookup key. Format:
   * `mdx://<collection>/<slug>` (no hash component — the JS stub does
   * not compile MDX, so it has no body hash to attach; the production
   * Rust-side `zfb-content::collection::Entry::module_specifier` adds a
   * `#<hash>` suffix and the bridge is responsible for matching either
   * form against its registered components).
   *
   * This field is part of the v0+ JS surface so the bridge has something
   * deterministic to key on without consulting per-call state.
   */
  module_specifier: string;
  /**
   * Renderable component for this entry.
   *
   * **Bridge contract.** At call time, `Content` consults
   * `globalThis.__zfb?.content?.get(entry.module_specifier)`. If the
   * bridge is present and returns a function, that function is invoked
   * with `props` and its result returned verbatim.
   *
   * **Fallback.** Outside the renderer (unit tests, dev sandboxes, or any
   * environment where `globalThis.__zfb.content.get` is absent or returns
   * `undefined`), `Content` returns a JSX-shaped element rendering the
   * raw markdown body inside a `<pre data-zfb-content-fallback>` block,
   * with a leading `[zfb fallback render]` marker line so the visual
   * distinction survives unstyled environments. The marker is also a
   * grep target for "did the production renderer not run?" diagnostics.
   *
   * **Typed signature.** Returns `ContentElement` (a structural alias for
   * `JSX.Element`) so consumers can drop `<entry.Content components={...} />`
   * into both React and Preact JSX without per-framework type setup.
   *
   * @example
   *   const post = (await getCollection("blog"))[0];
   *   return <post.Content components={{ ...defaultComponents, h1: MyH1 }} />;
   */
  Content: (props: ContentProps) => ContentElement;
};

// Cached node:fs / node:path module references. Populated lazily on first
// fs-path use (see [`loadNodeModules`]); reused on subsequent calls.
let cachedNodeFs: typeof NodeFs | undefined;
let cachedNodePath: typeof NodePath | undefined;

/**
 * Synchronously load `node:fs` and `node:path`, caching the results.
 *
 * The node specifiers are concatenated at runtime (`"node:" + "fs"`) so
 * esbuild's static analyzer cannot follow them — that's the load-bearing
 * detail here, because this module is reachable from browser-bundled
 * island chains via the `@takazudo/zfb` root barrel (see top-of-file note).
 *
 * Uses CommonJS `require` via [`createRequire`] (stable, sync) rather than
 * `await import()` (async, would force `getCollection` async and violate
 * ADR-004). `createRequire` itself is fetched from `node:module` through
 * the same runtime-built specifier pattern.
 *
 * If `createRequire` cannot be obtained at all (i.e. truly running in a
 * browser-shaped runtime — which would mean a misconfigured island
 * bundle), throws so the failure is loud rather than silent.
 */
function loadNodeModules(): { fs: typeof NodeFs; path: typeof NodePath } {
  if (cachedNodeFs !== undefined && cachedNodePath !== undefined) {
    return { fs: cachedNodeFs, path: cachedNodePath };
  }
  // Runtime-built specifiers: opaque to esbuild's static analyzer.
  const moduleSpecifier = "node:" + "module";
  const fsSpecifier = "node:" + "fs";
  const pathSpecifier = "node:" + "path";
  // Strategy A: prefer the host `require` from a CommonJS context. We
  // probe via `globalThis` and `Function`-built lookup so neither esbuild
  // nor stricter ESM tooling errors out at the lookup site.
  const dynamicGlobal = globalThis as unknown as { require?: NodeJS.Require };
  let nodeRequire: NodeJS.Require | undefined = dynamicGlobal.require;
  // Strategy B: ESM context — synthesize a require via `node:module`'s
  // `createRequire`. Loading `node:module` itself through the same
  // dynamic specifier shields it from esbuild's static walker.
  if (typeof nodeRequire !== "function") {
    // `Function("return require")()` returns the enclosing `require` when
    // the bundler/loader injects one (Node CJS, esbuild default). Falls
    // through if undefined — caught below.
    try {
      nodeRequire = new Function("return typeof require === 'function' ? require : undefined")() as
        | NodeJS.Require
        | undefined;
    } catch {
      nodeRequire = undefined;
    }
  }
  if (typeof nodeRequire !== "function") {
    // Last resort: synthesize via createRequire. Reaches `node:module`
    // through a dynamic require we have to bootstrap somehow — the only
    // way without a static `import` is `process.getBuiltinModule` (Node
    // 22+) which exposes built-ins synchronously without a require.
    const proc = (
      globalThis as unknown as { process?: { getBuiltinModule?: (id: string) => unknown } }
    ).process;
    const getBuiltin = proc?.getBuiltinModule;
    if (typeof getBuiltin === "function") {
      const mod = getBuiltin(moduleSpecifier) as typeof import("node:module");
      nodeRequire = mod.createRequire(import.meta.url);
    }
  }
  if (typeof nodeRequire !== "function") {
    throw new Error(
      "zfb/content: cannot load node:fs / node:path — no Node-style require available. " +
        "This module's filesystem path requires a Node runtime; if you see this in a browser " +
        "bundle, the bundler should externalize node:* imports (the islands bundler does so).",
    );
  }
  cachedNodeFs = nodeRequire(fsSpecifier) as typeof NodeFs;
  cachedNodePath = nodeRequire(pathSpecifier) as typeof NodePath;
  return { fs: cachedNodeFs, path: cachedNodePath };
}

/**
 * Resolve the directory that holds a named content collection. Override
 * via `ZFB_CONTENT_ROOT` so tests / fixtures can point at an arbitrary
 * directory.
 */
function resolveCollectionDir(name: string): string {
  const { path } = loadNodeModules();
  const envRoot = process.env["ZFB_CONTENT_ROOT"];
  const root = envRoot ? path.resolve(envRoot) : path.resolve(process.cwd(), "content");
  return path.join(root, name);
}

/**
 * Build the v0 stub's bridge specifier for an entry. Mirrors the Rust-side
 * convention (`mdx://<collection>/<slug>`) minus the body hash — the JS
 * stub does not compile MDX, so it has no hash to attach. The bridge
 * resolver on the renderer side is responsible for matching either form.
 */
function buildModuleSpecifier(collection: string, slug: string): string {
  return `mdx://${collection}/${slug}`;
}

/**
 * Build the `Content` component for an entry. Captures `module_specifier`
 * + `body` in the closure so the returned function takes only `props`.
 *
 * The bridge lookup is done lazily on every call (not at entry-construction
 * time) so the renderer can install / swap `globalThis.__zfb.content` at
 * any point before the first render without ordering hazards.
 */
function buildContentComponent(
  module_specifier: string,
  body: string,
): (props: ContentProps) => ContentElement {
  return function Content(props: ContentProps): ContentElement {
    const bridge = (globalThis as BridgeGlobal).__zfb?.content;
    const renderer = bridge?.get(module_specifier);
    if (typeof renderer === "function") {
      // Trust the bridge to return a JSX-element-shaped value — we don't
      // try to validate; both Preact and React JSX runtimes accept any
      // structural `{ type, props, key }` object on either side of the
      // boundary, and the renderer is the source of truth here.
      return renderer(props) as ContentElement;
    }
    return renderFallback(body);
  };
}

/**
 * Mint a content element through the per-project JSX runtime.
 *
 * Calls `jsx` from `react/jsx-runtime` — alias-rewritten to
 * `preact/jsx-runtime` in Preact mode by the engine (bundler.rs ~2886),
 * native in React mode — so the returned value is a real element for
 * whichever framework the project configured. This replaces the previous
 * hand-rolled `{ type, props, key, constructor: undefined }` object literal
 * (the Preact diff-path sentinel): that shape made `preact-render-to-string`
 * treat it as a VNode, but React's renderer rejects it as a child with
 * error #31 ("Objects are not valid as a React child") because a real React
 * element carries `$$typeof: Symbol.for("react.element")`. `children` is
 * passed inside `props` so a single child or an array both pass through
 * verbatim. Same migration as `Island` in this package. Kept private so
 * callers keep treating `ContentElement` / `ContentComponentElement` as
 * opaque. (Empty-MDX-body history: zudo-doc#505.)
 */
function mintElement(type: string, props: Record<string, unknown>): ContentElement {
  // `jsx`'s `type` param is typed `ElementType` (string-literal intrinsic
  // tags or component types), which rejects an arbitrary runtime `string`.
  // The tag is dynamic here, so cast to the factory's own first-param type —
  // robust whether the engine aliases `jsx` to react or preact at build time.
  return jsx(type as Parameters<typeof jsx>[0], props) as unknown as ContentElement;
}

/**
 * Build the structural JSX element returned when the bridge is absent.
 *
 * Shape: `<pre data-zfb-content-fallback>{marker}\n{body}</pre>` — the
 * leading `[zfb fallback render]` marker line is part of the public
 * fallback contract (it's both a visual signal and a grep target). Tests
 * pin both the attribute and the marker line.
 */
function renderFallback(body: string): ContentElement {
  return mintElement("pre", {
    "data-zfb-content-fallback": "",
    children: `${FALLBACK_MARKER}\n${body}`,
  });
}

/** Leading marker line emitted by [`renderFallback`]. Public contract. */
const FALLBACK_MARKER = "[zfb fallback render]";

/**
 * Load every `*.md` file in the named collection. Files starting with `.`
 * or that lack a `.md` extension are ignored.
 *
 * **ADR-004 contract: this function is synchronous.** TSX page modules
 * call it from anywhere — top-level, inside a render body, inside a
 * `useMemo` — and SSR completes in a single pass without yielding. The
 * snapshot path returns from memory; the filesystem fallback uses sync
 * `node:fs` APIs so the surface stays unified. (The legacy async
 * implementation was an oversight — the ADR predates it; SSG paths
 * always saw a Promise where ADR-004 says they should see an array,
 * which is why migrations from Astro tripped on `getCollection().filter
 * is not a function`.)
 *
 * @example
 *   const posts = getCollection<{ title: string; date: string }>("blog");
 */
export function getCollection<T = Record<string, unknown>>(name: string): CollectionEntry<T>[] {
  // Snapshot path: installed by `@takazudo/zfb-runtime`'s
  // `createPageRouter` at Worker boot. Worker runtimes have no `fs`, so
  // this branch is the production path under the embedded V8 host.
  //
  // The snapshot lookup reads `globalThis.__zfb.contentSnapshot`
  // (see `setContentSnapshot` above) rather than a per-module slot so
  // the cross-`zfb/content`-instance case under `--preserve-symlinks`
  // resolves through the same shared state — see #449.
  const installedSnapshot = (globalThis as SnapshotBridgeGlobal).__zfb?.contentSnapshot;
  if (installedSnapshot !== undefined) {
    const list = installedSnapshot.collections[name] ?? [];
    return list.map((entry) => entryFromSnapshot<T>(entry));
  }
  // Filesystem fallback (v0 path). Used by unit tests and direct Node
  // invocations outside the Worker bundle.
  //
  // BCI-6: traversal is now recursive — subdirectories are walked so a
  // collection rooted at `content/blog/` can contain nested `*.md` files
  // (e.g. `content/blog/2024/hello.md`). Slugs are derived from the
  // relative path so callers get stable, unique identifiers across nesting
  // levels.
  const dir = resolveCollectionDir(name);
  let mdPaths: string[];
  try {
    mdPaths = collectMdFilesSync(dir);
  } catch (err) {
    // Guard the `code` access at runtime — a thrown non-`Error` value
    // (rare, but possible) would otherwise crash here. We only swallow
    // a true ENOENT; anything else propagates.
    if (
      err !== null &&
      typeof err === "object" &&
      "code" in err &&
      (err as { code: unknown }).code === "ENOENT"
    ) {
      return [];
    }
    throw err;
  }
  const { fs, path } = loadNodeModules();
  return mdPaths.map((fullPath) => {
    const raw = fs.readFileSync(fullPath, "utf8");
    const { data, body } = parseFrontmatter(raw);
    // Derive a stable slug from the relative path (relative to collection
    // root), stripping the `.md` extension. For top-level files this
    // produces the same value as before; for nested files it produces a
    // path-based slug (e.g. `2024/hello`).
    const rel = path.relative(dir, fullPath);
    const slug = _relPathToSlug(rel);
    const module_specifier = buildModuleSpecifier(name, slug);
    return {
      slug,
      data: data as T,
      body,
      module_specifier,
      Content: buildContentComponent(module_specifier, body),
    };
  });
}

/**
 * Construct a [`CollectionEntry`] from a [`SnapshotEntry`]. The snapshot
 * carries `frontmatter` as a possibly-`null` JSON value (matches the
 * Rust contract for entries with no frontmatter); we normalise `null` /
 * `undefined` to an empty object so consumers' `.data.title` reads
 * never have to deal with `null`.
 *
 * **Type-safety note:** `T` is the caller-supplied frontmatter shape
 * but we do **not** validate it at runtime — if the page declares a
 * shape that the actual frontmatter doesn't match, the cast below
 * lies. Callers are expected to keep their `getCollection<MySchema>()`
 * generic in sync with the actual frontmatter; we acknowledge the
 * unsafety with the explicit `unknown` indirection rather than a
 * direct (and silently lossy) cast.
 */
function entryFromSnapshot<T>(entry: SnapshotEntry): CollectionEntry<T> {
  const data =
    entry.frontmatter === null || entry.frontmatter === undefined
      ? ({} as T)
      : (entry.frontmatter as unknown as T);
  return {
    slug: entry.slug,
    data,
    body: entry.body,
    module_specifier: entry.module_specifier,
    Content: buildContentComponent(entry.module_specifier, entry.body),
  };
}

/**
 * Recursively collect every `*.md` file under `dir` (synchronous).
 *
 * BCI-6: replaces the old flat `readdir(dir).filter(n => n.endsWith(".md"))`
 * approach. Hidden files (names starting with `.`) and hidden directories
 * are skipped at every nesting level, matching the top-level behaviour of
 * the previous implementation.
 *
 * Returns absolute paths sorted lexicographically so the result order is
 * deterministic across platforms and Node versions.
 *
 * Synchronous to honour ADR-004 — see [`getCollection`].
 */
function collectMdFilesSync(dir: string): string[] {
  const result: string[] = [];
  const { fs, path } = loadNodeModules();
  walkDirSync(fs, path, dir, result);
  result.sort();
  return result;
}

function walkDirSync(
  fs: typeof NodeFs,
  path: typeof NodePath,
  current: string,
  out: string[],
): void {
  const entries = fs.readdirSync(current, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.name.startsWith(".")) continue;
    const fullPath = path.join(current, entry.name);
    // Skip symlinks to avoid infinite loops caused by cycles (e.g. a symlink
    // pointing at a parent directory). Content files are expected to be plain
    // regular files; following symlinks provides no value here.
    if (entry.isSymbolicLink()) continue;
    if (entry.isDirectory()) {
      walkDirSync(fs, path, fullPath, out);
    } else if (entry.isFile() && entry.name.endsWith(".md")) {
      out.push(fullPath);
    }
  }
}

/**
 * @internal
 *
 * Convert a `path.relative()` result into a forward-slash-separated
 * slug with the trailing `.md` extension stripped.
 *
 * Slugs are URL-flavored identifiers, not filesystem paths — they
 * MUST use `/` regardless of the host OS so a nested entry like
 * `2024/hello.md` produces the slug `2024/hello` on both POSIX and
 * Windows. Without this normalisation, Windows callers would see
 * `2024\hello`, which then leaks through to `module_specifier` and
 * any URL the consumer derives from the slug.
 *
 * Exported solely so the unit test suite can pin the Windows
 * behaviour without needing an actual Windows host. Do not depend on
 * this from application code — name and signature may change.
 */
export function _relPathToSlug(relPath: string): string {
  const { path } = loadNodeModules();
  const posix = path.sep === "/" ? relPath : relPath.split(path.sep).join("/");
  // Some Node versions normalise `\` even when sep is `/`, so be
  // defensive: collapse any straggling backslashes too.
  const normalised = posix.includes("\\") ? posix.split("\\").join("/") : posix;
  return normalised.endsWith(".md") ? normalised.slice(0, -".md".length) : normalised;
}

// ---------------------------------------------------------------------------
// `defaultComponents` — htmlOverrides convention
//
// Ported from zudo-doc's `src/components/content/component-map.ts`. Users opt
// in by spreading the map into their own `components` prop:
//
//   import { defaultComponents } from "zfb";
//   <entry.Content components={{ ...defaultComponents, h2: MyH2 }} />
//
// Each component is a thin passthrough mirroring its zudo-doc counterpart
// (e.g. `ContentParagraph` → `<p {...rest}>{children}</p>`). v0 ships the
// passthroughs unstyled; layering smart-break / heading-anchor / link-icon
// behaviour on top is independent follow-up — keeping the v0 deliverable
// focused on infrastructure (issue #33).
//
// **`h1` is deliberately not in the map** — page titles render `<h1>` from
// frontmatter, per the zudo-doc convention. Adding `h1` here would silently
// double-render the page title.
//
// **Each override is exported as a named const AND included in
// `defaultComponents`** so consumers can tree-shake-import a single component
// (`import { ContentLink } from "zfb"`) without dragging in the whole map.
//
// Implementation note: components return the structural JSX-element shape
// directly — same pattern as `Island`. This keeps the package
// JSX-runtime-agnostic so it works under either Preact or React without
// importing a runtime. Both `jsx-runtime` implementations accept the
// `{ type, props, key }` object on either side of the boundary.
// ---------------------------------------------------------------------------

/**
 * Public JSX-element shape returned by every override in [`defaultComponents`].
 *
 * Mirrors [`ContentElement`] and [`IslandElement`]: a structural alias for
 * `JSX.Element` so consumers can drop these overrides into both React and
 * Preact JSX without per-framework type setup.
 */
export type ContentComponentElement = {
  readonly type: string;
  readonly props: Readonly<Record<string, unknown>>;
  readonly key: unknown;
};

/**
 * Props accepted by every default override. `children` and any extra
 * attributes (`className`, `id`, `href`, …) are passed through verbatim
 * to the underlying HTML element.
 */
export interface ContentComponentProps {
  children?: VNode;
  [key: string]: unknown;
}

/** Internal helper: build a structural JSX element of the given tag. */
function buildOverrideElement(tag: string, props: ContentComponentProps): ContentComponentElement {
  const { children, ...rest } = props;
  // Minted through the per-project JSX runtime (`mintElement`) so the
  // override is a real element under both React and Preact — see the
  // helper's docblock (zudo-doc#505; React error #31 rationale).
  return mintElement(tag, { ...rest, children }) as unknown as ContentComponentElement;
}

/**
 * `<h2>` passthrough override. Ported from zudo-doc's `HeadingH2`, stripped
 * of styling — v0 ships pass-through behaviour; visual treatment is layered
 * on by the consumer (or by a follow-up enhancement pass).
 */
export function ContentH2(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("h2", props);
}

/** `<h3>` passthrough override. See [`ContentH2`] for the contract. */
export function ContentH3(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("h3", props);
}

/** `<h4>` passthrough override. See [`ContentH2`] for the contract. */
export function ContentH4(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("h4", props);
}

/** `<p>` passthrough override. Mirrors zudo-doc's `ContentParagraph`. */
export function ContentParagraph(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("p", props);
}

/** `<a>` passthrough override. Mirrors zudo-doc's `ContentLink`. */
export function ContentLink(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("a", props);
}

/** `<strong>` passthrough override. Mirrors zudo-doc's `ContentStrong`. */
export function ContentStrong(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("strong", props);
}

/** `<blockquote>` passthrough override. Mirrors zudo-doc's `ContentBlockquote`. */
export function ContentBlockquote(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("blockquote", props);
}

/** `<ul>` passthrough override. Mirrors zudo-doc's `ContentUl`. */
export function ContentUl(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("ul", props);
}

/** `<ol>` passthrough override. Mirrors zudo-doc's `ContentOl`. */
export function ContentOl(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("ol", props);
}

/** `<table>` passthrough override. Mirrors zudo-doc's `ContentTable`. */
export function ContentTable(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("table", props);
}

/** `<code>` passthrough override. Mirrors zudo-doc's `ContentCode`. */
export function ContentCode(props: ContentComponentProps): ContentComponentElement {
  return buildOverrideElement("code", props);
}

/**
 * Default per-element override map — eleven entries covering the markdown
 * tags the zudo-doc convention overrides (`h2`, `h3`, `h4`, `p`, `a`,
 * `strong`, `blockquote`, `ul`, `ol`, `table`, `code`).
 *
 * `h1` is intentionally absent: page titles render from frontmatter, per
 * the zudo-doc convention.
 *
 * Spread into a `components` prop to compose with custom overrides:
 *
 * ```tsx
 * import { defaultComponents } from "zfb";
 *
 * <entry.Content components={{ ...defaultComponents, h2: MyFancyH2 }} />
 * ```
 */
export const defaultComponents = {
  h2: ContentH2,
  h3: ContentH3,
  h4: ContentH4,
  p: ContentParagraph,
  a: ContentLink,
  strong: ContentStrong,
  blockquote: ContentBlockquote,
  ul: ContentUl,
  ol: ContentOl,
  table: ContentTable,
  code: ContentCode,
} as const;
