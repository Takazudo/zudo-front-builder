# Z0 Decision Record — Browser client-resource companions for md-wasm

_Decision output for [issue #1638](https://github.com/Takazudo/zudo-front-builder/issues/1638), part of [epic #1634](https://github.com/Takazudo/zudo-front-builder/issues/1634). This is the implementation contract for issues #1639, #1640, and #1641. It deliberately changes no product behavior on its own._

## Status

**Accepted.** Use a browser-conditional package entry with statically imported, typed resource URLs, plus a generic zfb-islands resource-companion class. Do not make a package-specific islands exception and do not expose a consumer URL or initialization API.

## TL;DR

- Keep the ordinary package root entry for Node and direct-module use. Add a browser conditional root entry with the same public exports.
- The browser entry imports generated wasm-bindgen glue as an explicit \*.zfb-resource.mjs URL and imports the .wasm as an explicit URL. It dynamically imports the emitted glue URL with its existing bounded ?zfbMdWasmGen=<generation> query.
- zfb browser esbuild owns --loader:.zfb-resource.mjs=file, --loader:.wasm=file, and --asset-names=islands-resource-[name]-[hash]. These built-ins are not exposed through the user loader map.
- The islands bundler reads file-loader outputs from esbuild's metafile into a separate typed resources collection. It ships validated resources verbatim beside the island entry; no later stage may rename them.
- Preserve one compiled WebAssembly.Module. A genuine trap drops the poisoned instance and uses a fresh query-versioned glue module record. The recovery cap remains bounded.

---

## 1. Scope and baseline

The current wrapper resolves generated glue and wasm relative to import.meta.url. It caches a compiled module and recovers a genuine wasm trap by dynamically importing glue with a generation query, because wasm-bindgen web glue keeps its initialized instance in module scope. That recovery behavior remains required.

The gap is browser-island delivery. The dynamic glue specifier is intentionally opaque to static bundling, while islands read-back currently accepts only its known JavaScript entry, chunks, and workers. Preserving a new URL expression does not create a validated copied resource or cause production/dev writers to serve it.

This record decides only delivery. It does not add highlightCode, alter the Rust/WASM API, create a slim artifact, or change the current recovery cap.

| Term          | Meaning                                                                                                                |
| ------------- | ---------------------------------------------------------------------------------------------------------------------- |
| direct entry  | Default package entry used by Node and an unbundled package directory served to a browser.                             |
| browser entry | Root entry selected by a browser-aware bundler through the browser export condition. It has the exact same public API. |
| resource      | A non-entry file emitted through one of the two locked esbuild file loaders. It is neither a chunk nor a worker.       |
| glue resource | The generated wasm-bindgen JavaScript copied as \*.zfb-resource.mjs so it remains dynamically importable JavaScript.   |
| generation    | Wrapper-private monotonic integer appended after a real trap. It is bounded and never caller input.                    |

---

## 2. Alternatives

### A. Bundler-safe wasm-bindgen factory

**Rejected as the whole solution.** A factory could make instantiation explicit, but it does not itself ship the glue and wasm files or create a fresh ES module record after a trap. Forking/reworking generated glue would add wasm-bindgen-specific maintenance while still requiring generic resource transport.

### B. Browser package entry plus generic resource companions

**Selected.** The package identifies its two generated files through static imports. zfb turns those imports into URL resources and carries them through a generic resource type. The package retains query-versioned recovery and its default Node/direct entry.

### C. Copy the current nested glue/WASM tree

**Rejected.** Directory copying cannot prove the closure of dynamic imports, preserve every import.meta.url relationship after output renaming, or provide a small trustworthy naming/collision boundary. It makes dev cleanup depend on a package layout instead of esbuild's actual output graph.

### D. Consumer-supplied URLs or init

**Rejected.** A bare lazy import can be self-contained. Requiring an application to locate package internals would create a deployment API solely to work around bundling and would make recovery/base-path behavior consumer-owned.

### E. Embed wasm in ordinary island JavaScript

**Rejected.** The package documentation records approximately 2.9 MB raw / 1.3 MB gzip wasm. Inlining would inflate parsed client script, weaken independent caching, and make fresh glue recovery harder to reason about. No measurement shows it is superior.

---

## 3. Locked package contract (issue #1639)

### 3.1 Root export resolution

The published root export has this shape; development exports mirror it with source paths:

```json
{
  "exports": {
    ".": {
      "types": "./dist/index.d.ts",
      "browser": "./dist/browser.js",
      "default": "./dist/index.js"
    }
  }
}
```

Both entries export identical public functions, classes, test hooks, and types. The condition changes only internal resource acquisition. Browser is not a new public subpath; a user keeps writing:

```ts
const md = await import("@takazudo/zfb-md-wasm");
```

| Consumer                      | Entry      | Required behavior                                                                                    |
| ----------------------------- | ---------- | ---------------------------------------------------------------------------------------------------- |
| zfb islands, browser platform | browser.js | Static glue/wasm resource edges; URLs resolve next to emitted island output.                         |
| Node                          | index.js   | Retain node:fs/promises loading for file: wasm URL and dynamic glue import.                          |
| Direct served package module  | index.js   | Retain fetch/module-relative behavior; host serves .mjs as JavaScript and .wasm as application/wasm. |
| SSR / non-browser bundle      | index.js   | Never selects browser resources.                                                                     |

Factor the wrapper into shared code that accepts glue/wasm URLs (or equivalent loaders). Entry-specific code only supplies URLs and the direct-entry Node-versus-fetch byte source; it must not duplicate the public API or recovery algorithm.

### 3.2 Generated and published files

The packed artifact must contain at least:

| File                                                                              | Purpose                                                          |
| --------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| dist/index.js and dist/index.d.ts                                                 | Direct/Node root entry and public types.                         |
| dist/browser.js                                                                   | Browser-conditional root entry with the same API.                |
| dist/wasm/zfb_md_wasm_glue.zfb-resource.mjs                                       | wasm-bindgen web glue marked for zfb's built-in resource loader. |
| dist/wasm/zfb_md_wasm_glue.zfb-resource.d.mts, or TypeScript's equivalent sidecar | Generated glue declaration surface.                              |
| dist/wasm/zfb_md_wasm_bg.wasm                                                     | Optimized wasm payload.                                          |

The marked .mjs is the one canonical glue runtime file. Do not publish a second divergent generated glue copy merely for the conditional entry. The marker is package-to-zfb integration metadata, not a public import path.

### 3.3 Browser URL expression

The browser entry has this locked semantic shape:

```ts
import glueHref from "./wasm/zfb_md_wasm_glue.zfb-resource.mjs";
import wasmHref from "./wasm/zfb_md_wasm_bg.wasm";

const glueUrl = new URL(glueHref, import.meta.url);
const wasmUrl = new URL(wasmHref, import.meta.url);
```

The imported strings are emitted by esbuild's file loader, not source-tree paths. They are resolved from the final island entry. If production renames islands.js to islands-<entry-hash>.js, both resource URLs still resolve in the same assets directory. Package code must not replace them with an assumed absolute /assets URL.

For recovery the shared wrapper dynamically imports glueUrl.href with one query parameter:

```ts
import(glueUrl.href + "?zfbMdWasmGen=" + generation);
```

Append the query only after forming the emitted glue URL. Do not cache-bust the wasm URL. The wrapper compiles/fetches wasm once and calls glue.initSync with that cached module.

---

## 4. Locked zfb-islands resource contract (issue #1640)

### 4.1 Esbuild arguments and names

Browser-island bundling owns these non-operator-configurable flags in addition to existing flags:

```text
--loader:.zfb-resource.mjs=file
--loader:.wasm=file
--asset-names=islands-resource-[name]-[hash]
--metafile=<staging-outdir>/meta.json
```

The longest extension rule makes foo.zfb-resource.mjs a file resource instead of ordinary bundled JavaScript. The file loader preserves the final .mjs extension, so a static server can give copied glue JavaScript MIME. The wasm rule creates a separately emitted binary URL.

Do not admit file or copy into BundleConfig.loaders or user config. The user loader allow-list remains non-emitting. The two built-ins are generic but explicit: any island can use .wasm or opt into \*.zfb-resource.mjs; zfb-islands must not name or inspect @takazudo/zfb-md-wasm internals.

The command/config validator rejects operator loader keys .wasm and .zfb-resource.mjs, and the fixed rules are emitted after any allowed operator loader arguments. An application therefore cannot accidentally override this resource contract with empty, dataurl, or another loader.

All output names are flat:

```text
entry:     islands.js
chunk:     islands-chunk-<hash>.js
worker:    worker-<encoded-source>.js
resource:  islands-resource-<name>-<hash>.<original-final-extension>
```

Representative resource names are islands-resource-zfb_md_wasm_glue.zfb-resource-<hash>.mjs and islands-resource-zfb_md_wasm_bg-<hash>.wasm.

### 4.2 Typed read-back and metafile oracle

Keep resources separate through the islands boundary:

```text
BundleOutput { bytes, chunks, workers, resources }
ProductionIslandsAsset { bytes, chunks, workers, resources }
```

The final Rust type names may vary, but resources must remain a distinct deterministic collection of { filename, bytes }, not disguised as chunks/workers.

Read-back accepts a resource only when all conditions hold:

1. It is a regular top-level staging output with a UTF-8 flat basename.
2. It matches the reserved islands-resource name class and expected .mjs or .wasm final extension.
3. Esbuild's metafile traces it to a file-loader edge whose source ends in .zfb-resource.mjs or .wasm.
4. Its filename is unique across the entry, chunks, workers, sourcemaps, and resources.

Fail closed before transporting bytes for a directory, non-UTF-8 name, separator, backslash, .., unknown prefix, malformed extension, missing entry, unbacked metafile output, duplicate, or cross-class collision. Sort resources by filename after validation. Sourcemaps remain ignored only as mapped siblings of an already recognized JS entry/chunk/worker; a .map is never a resource.

### 4.3 Compatibility

- A Wasm-free island produces resources = [] and preserves existing entry/chunk/worker behavior byte-for-byte.
- Discovery is scoped to browser island output. It does not modify SSR bundling, CSS, or arbitrary client-script policy.
- A synthetic glue/WASM fixture is sufficient for the islands tests; it must not depend on a full md-wasm build.

---

## 5. Production and dev lifecycle (issue #1641)

### 5.1 Production

After the islands classification is validated, map chunks, workers, and resources to the existing verbatim companion writer. The production result is:

```text
dist/assets/islands-<entry-hash>.js
dist/assets/islands-chunk-<hash>.js
dist/assets/worker-<encoded-source>.js
dist/assets/islands-resource-<name>-<hash>.mjs
dist/assets/islands-resource-<name>-<hash>.wasm
```

The pipeline may hash the entry as it does today. It writes every companion name and byte sequence verbatim: never add another hash, strip the marker, move a resource to a nested directory, or rewrite bytes after esbuild baked the relative URL. The final filesystem boundary validates the unified companion-name set again and rejects traversal, empty names, duplicates, and cross-class collisions.

### 5.2 Development

Dev retains assets/islands.js and uses the same self-hashed flat resource filenames. Track one complete previous companion filename set for chunks, workers, and resources.

For each successful generation:

1. Validate the complete typed payload and unified name set.
2. Write all new/changed companions to assets/.
3. Write islands.js only after its referenced companions exist.
4. Prune only names in the previous tracked set that are absent from the new set.

Ignore a not-found prune, but retain existing reporting for another delete failure. With no island bundle, do no resource writes; when transitioning to no islands, remove only the tracked resource set. Never recursively delete assets/ or unrelated application assets.

### 5.3 Serving, base paths, and SSR

Existing static asset serving must return JavaScript MIME for .mjs and application/wasm for .wasm. A glue URL query changes ES module identity, not asset selection, so ?zfbMdWasmGen=N serves the same copied glue bytes.

Relative resource URLs work under the hashed production entry and custom base paths without runtime string rewriting. SSR always resolves the default entry; no browser resource, browser fetch, or client companion enters an SSR bundle.

---

## 6. Recovery invariants

1. The first successful load fetches/reads the emitted wasm resource and creates one cached WebAssembly.Module.
2. A WebAssembly.RuntimeError drops the poisoned instance. Replacement dynamically imports the emitted glue URL with a new zfbMdWasmGen query and runs initSync with the cached module. Calling initSync again on old glue is not recovery.
3. Concurrent trap reporters for one poisoned generation single-flight to one replacement.
4. Preserve the current bounded cap, 16 at this decision point. Once reached, retain the terminal recovery error instead of growing module records indefinitely.
5. Structured input diagnostics do not throw a trap error or poison the cached instance.
6. Node/direct behavior remains compatible; only the browser condition uses static resource URL imports.

---

## 7. Release-artifact proof

Package validation must exercise pnpm pack output, not just source or dist. A pack-content assertion fails closed unless the tarball contains the direct entry, browser entry, public declarations, one marked glue .mjs plus declaration sidecar, and one wasm payload at the documented paths.

A generic-esbuild fixture resolves the packed artifact's root under browser conditions and proves:

- browser.js, not the direct entry, was selected;
- one copied .mjs resource and one copied .wasm resource exist under the reserved islands-resource prefix;
- the emitted entry references both relatively;
- a forced trap makes the next call use a distinct query-versioned glue resource while compiled-module count stays one.

The later Chromium lane consumes that packed-shaped fixture. It must not substitute source imports or manually copy package assets.

---

## 8. Focused evidence

A disposable esbuild 0.25.12 probe was run outside committed product files. It statically imported a synthetic compound-extension glue module and wasm, then ran:

```sh
pnpm dlx esbuild@0.25.12 <probe>/src/entry.mjs \
  --bundle --format=esm --splitting --outdir=<probe>/out \
  --metafile=<probe>/meta.json \
  --loader:.wasm=file --loader:.wasm-bindgen.mjs=file \
  '--asset-names=islands-resource-[name]-[hash]'
```

Result: entry.js, islands-resource-zfb_md_wasm_glue.wasm-bindgen-<hash>.mjs, and islands-resource-zfb_md_wasm_bg-<hash>.wasm. The entry contained relative resource strings and new URL(glueHref, import.meta.url); the metafile recorded both output edges as file-loader inputs. The final \*.zfb-resource.mjs marker uses the same longest-extension file-loader mechanism but gives zfb a generic, explicit opt-in.

The emitted probe then loaded glue with generation 0, forced a WebAssembly.RuntimeError, and loaded generation 1. Its output was:

```json
{ "compiledModuleLoads": 1, "generations": ["0", "1"], "freshModuleRecord": true }
```

This proves static file-loader discovery and fresh query-versioned module records without wasm recompilation. The probe and all generated output were removed before this record.

---

## 9. Required downstream validation

| Owner          | Must prove                                                                                                                                                              |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Package work   | Browser root condition, shared API, direct/Node compatibility, marked glue/WASM generation, generic-esbuild fixture, forced-trap recovery, and tarball assertion.       |
| Islands work   | Built-in loader args, metafile-backed typed resources, deterministic ordering, zero-resource compatibility, and fail-closed malformed/traversal/collision tests.        |
| Lifecycle work | Production sibling writes with hashed entry/custom base path, dev write-before-entry plus stale deletion, no-island/SSR compatibility, and final filesystem validation. |
| Browser work   | Packed artifact in a real zfb island; 200 glue/wasm responses with correct MIME; semantic output/fallback; forced-trap query-versioned recovery.                        |

Changing the marker extension, output naming, URL relativity, conditional-export behavior, or recovery invariant above is an architecture change and requires updating this decision record first.
