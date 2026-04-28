// `zfb/content` — minimal v0 content collection loader.
//
// Reads `*.md` files from a content collection directory, parses YAML
// frontmatter, and returns typed entries. This is a deliberately small
// stub so the basic-blog example can call `getCollection("blog")` today;
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

import { readdir, readFile } from "node:fs/promises";
import { join, relative, resolve } from "node:path";

import type { VNode } from "./jsx-types.js";

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
 * Module-level snapshot slot. `undefined` means "no snapshot installed";
 * `getCollection` falls back to the Node `fs` path. The runtime package
 * sets this once during init and (in dev mode) overwrites it on each
 * rebuild.
 */
let installedSnapshot: Snapshot | undefined;

/**
 * Register a [`Snapshot`] so [`getCollection`] resolves from memory.
 *
 * Pass `undefined` to clear (used by tests that need to restore the v0
 * filesystem path between runs). Idempotent: the latest call wins.
 */
export function setContentSnapshot(snapshot: Snapshot | undefined): void {
  installedSnapshot = snapshot;
}

/**
 * Read the currently-installed [`Snapshot`], or `undefined` if none is
 * registered. Exposed mostly for tests; production callers should not
 * need to introspect the bridge state.
 */
export function getContentSnapshot(): Snapshot | undefined {
  return installedSnapshot;
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

/**
 * Resolve the directory that holds a named content collection. Override
 * via `ZFB_CONTENT_ROOT` so tests / fixtures can point at an arbitrary
 * directory.
 */
function resolveCollectionDir(name: string): string {
  const envRoot = process.env["ZFB_CONTENT_ROOT"];
  const root = envRoot ? resolve(envRoot) : resolve(process.cwd(), "content");
  return join(root, name);
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
 * Build the structural JSX element returned when the bridge is absent.
 *
 * Shape: `<pre data-zfb-content-fallback>{marker}\n{body}</pre>` — the
 * leading `[zfb fallback render]` marker line is part of the public
 * fallback contract (it's both a visual signal and a grep target). Tests
 * pin both the attribute and the marker line.
 */
function renderFallback(body: string): ContentElement {
  return {
    type: "pre",
    props: {
      "data-zfb-content-fallback": "",
      children: `${FALLBACK_MARKER}\n${body}`,
    },
    key: null,
  };
}

/** Leading marker line emitted by [`renderFallback`]. Public contract. */
const FALLBACK_MARKER = "[zfb fallback render]";

/**
 * Load every `*.md` file in the named collection. Files starting with `.`
 * or that lack a `.md` extension are ignored.
 *
 * @example
 *   const posts = await getCollection<{ title: string; date: string }>("blog");
 */
export async function getCollection<T = Record<string, unknown>>(
  name: string,
): Promise<CollectionEntry<T>[]> {
  // Snapshot path: installed by `@takazudo/zfb-runtime`'s
  // `createPageRouter` at Worker boot. Worker runtimes have no `fs`, so
  // this branch is the production path under miniflare.
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
    mdPaths = await collectMdFiles(dir);
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
  const entries = await mapWithConcurrency(mdPaths, READ_CONCURRENCY, async (fullPath) => {
    const raw = await readFile(fullPath, "utf8");
    const { data, body } = parseFrontmatter(raw);
    // Derive a stable slug from the relative path (relative to collection
    // root), stripping the `.md` extension. For top-level files this
    // produces the same value as before; for nested files it produces a
    // path-based slug (e.g. `2024/hello`).
    const rel = relative(dir, fullPath);
    const slug = rel.slice(0, -".md".length);
    const module_specifier = buildModuleSpecifier(name, slug);
    return {
      slug,
      data: data as T,
      body,
      module_specifier,
      Content: buildContentComponent(module_specifier, body),
    };
  });
  return entries;
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

/** Maximum concurrent file reads while loading a content collection. */
const READ_CONCURRENCY = 8;

/**
 * Recursively collect every `*.md` file under `dir`.
 *
 * BCI-6: replaces the old flat `readdir(dir).filter(n => n.endsWith(".md"))`
 * approach. Hidden files (names starting with `.`) and hidden directories
 * are skipped at every nesting level, matching the top-level behaviour of
 * the previous implementation.
 *
 * Returns absolute paths sorted lexicographically so the result order is
 * deterministic across platforms and Node versions.
 */
async function collectMdFiles(dir: string): Promise<string[]> {
  const result: string[] = [];
  await walkDir(dir, result);
  result.sort();
  return result;
}

async function walkDir(current: string, out: string[]): Promise<void> {
  const entries = await readdir(current, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.name.startsWith(".")) continue;
    const fullPath = join(current, entry.name);
    // Skip symlinks to avoid infinite loops caused by cycles (e.g. a symlink
    // pointing at a parent directory). Content files are expected to be plain
    // regular files; following symlinks provides no value here.
    if (entry.isSymbolicLink()) continue;
    if (entry.isDirectory()) {
      await walkDir(fullPath, out);
    } else if (entry.isFile() && entry.name.endsWith(".md")) {
      out.push(fullPath);
    }
  }
}

/**
 * Run `fn` over `items` with at most `limit` invocations in flight at any
 * time. Preserves input order in the returned array. Implemented inline so
 * we don't pull in a `p-limit`-shaped dependency for one call site.
 */
async function mapWithConcurrency<I, O>(
  items: readonly I[],
  limit: number,
  fn: (item: I, index: number) => Promise<O>,
): Promise<O[]> {
  const cap = Math.max(1, Math.min(limit, items.length));
  const out = new Array<O>(items.length);
  let cursor = 0;
  async function worker(): Promise<void> {
    while (true) {
      const i = cursor++;
      if (i >= items.length) return;
      out[i] = await fn(items[i] as I, i);
    }
  }
  const workers: Promise<void>[] = [];
  for (let w = 0; w < cap; w++) workers.push(worker());
  await Promise.all(workers);
  return out;
}

// ---------------------------------------------------------------------------
// Minimal frontmatter parser. Intentionally NOT a full YAML implementation —
// the v0 surface is documented above. This avoids pulling in `gray-matter`
// or `js-yaml` for what is, in v0, three field types.
// ---------------------------------------------------------------------------

const FRONTMATTER_DELIM = "---";

export type ParsedFrontmatter = {
  data: Record<string, unknown>;
  body: string;
};

/**
 * Parse a leading YAML-ish frontmatter block off a markdown document.
 *
 * **Public SDK surface.** Re-exported from `zfb/content` so consumers can
 * write their own custom content loaders without re-implementing the
 * (deliberately minimal) v0 frontmatter parser. The accepted grammar is
 * documented at the top of this module.
 *
 * Handles:
 * - empty frontmatter (`---\n---\nbody`) → `{ data: {}, body: "body" }`
 * - file ending exactly with `---` (no trailing newline) → frontmatter
 *   parsed, body is empty.
 *
 * Returns `{ data: {}, body: <input> }` unchanged when no frontmatter
 * fence is present, or when the opening fence has no matching closer.
 */
export function parseFrontmatter(raw: string): ParsedFrontmatter {
  // Strip a leading BOM and normalise line endings before splitting.
  const text = raw.replace(/^﻿/, "").replace(/\r\n/g, "\n");
  if (!text.startsWith(`${FRONTMATTER_DELIM}\n`)) {
    return { data: {}, body: text };
  }
  const headerStart = FRONTMATTER_DELIM.length + 1; // after first "---\n"

  // Search for the closing fence. Accept either `\n---\n` (frontmatter
  // followed by body) or `\n---` at the very end of the document
  // (frontmatter with no trailing newline). Start the search at
  // `headerStart - 1` so the empty-frontmatter case `---\n---\n...`
  // is detected (the `\n---` at index 3 immediately follows the opener).
  const searchFrom = headerStart - 1;
  let closeIdx = -1;
  let bodyStart = -1;
  let i = searchFrom;
  while (i <= text.length - `\n${FRONTMATTER_DELIM}`.length) {
    const candidate = text.indexOf(`\n${FRONTMATTER_DELIM}`, i);
    if (candidate === -1) break;
    const afterFence = candidate + `\n${FRONTMATTER_DELIM}`.length;
    if (afterFence === text.length) {
      // `\n---` at end-of-string — frontmatter ends here, body is empty.
      closeIdx = candidate;
      bodyStart = afterFence;
      break;
    }
    if (text.charAt(afterFence) === "\n") {
      // `\n---\n` — body starts after the trailing newline.
      closeIdx = candidate;
      bodyStart = afterFence + 1;
      break;
    }
    // `\n---` followed by more `-` (e.g. `\n----`) — keep searching.
    i = candidate + 1;
  }
  if (closeIdx === -1 || bodyStart === -1) {
    // Malformed frontmatter (no closing delimiter): treat as plain body.
    return { data: {}, body: text };
  }
  const header = text.slice(headerStart, closeIdx);
  const body = text.slice(bodyStart);
  return { data: parseFrontmatterHeader(header), body };
}

function parseFrontmatterHeader(header: string): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const lines = header.split("\n");
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    if (line.trim() === "" || line.trimStart().startsWith("#")) {
      i++;
      continue;
    }
    // Top-level keys are unindented `key: value` or `key:` (then list).
    const m = /^([A-Za-z_][\w-]*)\s*:\s*(.*)$/.exec(line);
    if (!m) {
      i++;
      continue;
    }
    const key = m[1] as string;
    const inlineValue = (m[2] ?? "").trim();
    if (inlineValue === "") {
      // Possible block list.
      const list: string[] = [];
      let j = i + 1;
      while (j < lines.length) {
        const next = lines[j] ?? "";
        const itemMatch = /^\s*-\s+(.*)$/.exec(next);
        if (!itemMatch) break;
        list.push(unwrapScalar((itemMatch[1] ?? "").trim()));
        j++;
      }
      if (list.length > 0) {
        out[key] = list;
        i = j;
        continue;
      }
      // Empty value with no list — record empty string for completeness.
      out[key] = "";
      i++;
      continue;
    }
    out[key] = unwrapScalar(inlineValue);
    i++;
  }
  return out;
}

function unwrapScalar(value: string): string {
  if (value.length >= 2) {
    const first = value.charAt(0);
    const last = value.charAt(value.length - 1);
    if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
      return value.slice(1, -1);
    }
  }
  return value;
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
  return {
    type: tag,
    props: { ...rest, children },
    key: null,
  };
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
