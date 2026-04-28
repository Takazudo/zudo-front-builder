# ADR-004: Content bridge contract (Rust → JS `getCollection` / `getEntry`)

- **Status:** Accepted (Rust + d.ts side; JS-runtime wiring deferred)
- **Date:** 2026-04-27
- **Owners:** Sub 48 (Engine Primitives — content query contract)
- **Related:** ADR-001 (JS runtime selection — superseded by ADR-005;
  the `deno_core` implementation has been replaced with a Node-hosted
  miniflare worker), Epic #42 (Engine Primitives)

## Context

zfb's TSX page modules need a synchronous way to query content
collections during SSR — equivalent to Astro's `getCollection("docs")`
or `getEntry("docs", "intro")`. The data lives on disk as Markdown,
MDX, or TSX files; the JS runtime cannot touch the filesystem (the
SSR sandbox doesn't get FS access). The build must therefore read
every collection up-front in Rust, freeze it into a serializable
snapshot, and embed that snapshot into the JS runtime's `globalThis`
before any TSX module executes.

This ADR fixes the contract for that snapshot — both the Rust types
and the TypeScript surface — so the deno_core implementer (separate
follow-up epic, gated by ADR-001) has a spec to code against.

## Decision

A single deterministic `ContentSnapshot` is built in Rust at the start
of every render pass and exposed to JS through `globalThis.__zfb.content`.
TSX page modules consume it via the `zfb/content` module declared in
`.zfb/types.d.ts`.

### Rust contract

`crates/zfb-content/src/content_bridge.rs`:

```rust
pub struct EntrySnapshot {
    pub slug: String,
    pub frontmatter: serde_json::Value,
    pub body: String,
    pub module_specifier: String,
    pub rel_path: String,
}

pub struct ContentSnapshot {
    pub collections: BTreeMap<String, Vec<EntrySnapshot>>,
}

pub struct CollectionConfig {
    pub name: String,
    pub root: PathBuf,
}

pub fn build_snapshot(
    collections: &[CollectionConfig],
) -> Result<ContentSnapshot, BridgeError>;
```

All four structs derive `Debug, Clone, Serialize, Deserialize` so the
snapshot round-trips cleanly through `serde_json`.

### Determinism

`build_snapshot` is **deterministic**: identical input always produces
byte-identical output.

- Top-level keys: collections are stored in a `BTreeMap<String, _>`.
  `BTreeMap` iterates in sorted-key order, so collection names appear
  ascending in the serialized JSON.
- Per-collection entries: `Vec<EntrySnapshot>` is explicitly sorted by
  `slug` ascending after the walker returns. (The walker sorts by
  `rel_path`, which can disagree with slug ordering when files live
  in nested directories — the bridge re-sorts to match the contract.)

The combined sort key is **`(collection_name, slug)` ascending**. The
determinism test hashes `serde_json::to_string(&snapshot)` with
SHA-256 and asserts repeated calls produce the same digest.

### Missing collection roots

A `CollectionConfig` whose `root` does not exist on disk yields an
empty `Vec` (no error). This matches `walk_collection` /
`collect_collection_files`. It allows configs to reference optional
collections (e.g. an i18n locale that hasn't been authored yet)
without forcing the build to fail.

### JS-side contract

The runtime serializes `ContentSnapshot` to JSON and embeds it on
`globalThis.__zfb.content` at startup, **before any TSX module
evaluates**:

```ts
// Internal runtime shape — TSX modules import the typed `zfb/content`
// surface below instead of touching this directly.
globalThis.__zfb = {
  content: {
    /** Synchronous. Returns the array of entries for the named collection.
     *  Returns an empty array if the collection is unknown. */
    get(name: string): Entry[];

    /** Synchronous. Returns the matching entry, or `undefined`. */
    getOne(name: string, slug: string): Entry | undefined;
  },
};
```

The `Entry` shape mirrors `EntrySnapshot` minus internal fields the JS
side does not need — `slug`, `data` (renamed from `frontmatter` for
parity with Astro), and `body`. The full `module_specifier` and
`rel_path` fields stay available for tooling that needs them but are
not part of the typed user-facing surface.

### TypeScript surface

`emit_types_dts` writes `.zfb/types.d.ts` containing:

```ts
declare module "zfb/content" {
  export interface ZfbCollections {
    blog: { slug: string; data: Record<string, unknown>; body: string };
    docs: { slug: string; data: Record<string, unknown>; body: string };
    // ...one per registered collection
  }
  export function getCollection<K extends keyof ZfbCollections>(
    name: K,
  ): ZfbCollections[K][];
  export function getEntry<K extends keyof ZfbCollections>(
    name: K,
    slug: string,
  ): ZfbCollections[K] | undefined;
}
```

The legacy `zfb-collections` module declaration is preserved verbatim
in the same `.d.ts` for backward compatibility with existing tooling.

The deno_core wiring (separate epic) is responsible for installing
`getCollection` / `getEntry` runtime implementations that read from
`globalThis.__zfb.content.get` / `.getOne`.

### Synchronous-only

`getCollection` and `getEntry` are **synchronous**. They return Plain
Old Data already loaded into memory at startup; there is no async
boundary, no I/O, no `await`. This is non-negotiable: SSR rendering
must complete in one pass without yielding the runtime, and TSX page
components must be allowed to call these helpers from anywhere
(top-level, inside a component, inside a `useMemo`, etc.).

## Out of scope (this ADR)

- **deno_core wiring.** Embedding `ContentSnapshot` into a V8 isolate
  via `op_state` / `OpState` extensions. Lands in the ADR-001
  follow-up epic; the snapshot type is now the stable interface
  between the two halves.
- **Schema-aware `data` types.** `data` is typed as
  `Record<string, unknown>` in the emitted d.ts. A future sub will
  reflect the Rust collection schema (`T`) into a precise TS
  interface — that change is contained inside `emit_types_dts` and
  does not affect this contract.
- **Live reload.** The dev server re-runs `build_snapshot` on every
  edit; how the runtime swaps in a new snapshot without restarting
  the isolate is the deno_core epic's problem.

## Consequences

- TSX pages can query collections with the same ergonomics as Astro
  without paying for an async runtime barrier.
- The snapshot is bounded by RAM; very large content sets (10k+
  entries) will need a streaming or paged variant, but the v1 target
  (~hundreds of entries per collection) fits comfortably.
- The Rust → JS interface is one JSON blob: stable, debuggable,
  diffable. Any bridge regression shows up as a hash mismatch in the
  determinism test.
- Adding new entry-level fields (e.g. computed reading time) is a
  pure additive change to `EntrySnapshot`; downstream JS keeps
  compiling because the d.ts uses `Record<string, unknown>` for
  `data` and explicit fields elsewhere.
