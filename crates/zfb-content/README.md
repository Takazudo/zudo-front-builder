# zfb-content

Markdown/MDX pipeline, frontmatter parsing, syntect syntax highlighting, and content collections for the zfb framework.

## Architecture

The crate is organized around four concerns that compose into the full content pipeline:

### 1. Markdown/MDX pipeline (`pipeline`, `plugins`, `serializer`)

`pipeline` implements a two-phase AST pipeline:

1. Parse source → `markdown::mdast::Node` (mdast) using `markdown::to_mdast` with MDX-aware options.
2. Run `MdastVisitor`s over the mdast tree (mutation).
3. Transform mutated mdast → `HastNode` (a lightweight HTML AST defined in-crate, mirroring the unified `remark → rehype` split).
4. Run `HastVisitor`s over the hast tree (mutation).

`plugins` contains Rust ports of zudo-doc's md-plugins as `MdastVisitor` / `HastVisitor` implementations.

`serializer` turns a `HastNode` tree into an HTML fragment string (mirrors `hast-util-to-html`; intentionally minimal — no whitespace prettification, no DOCTYPE).

### 2. MDX → JSX emitter (`mdx_jsx_emit`)

Turns an MDX source string into a self-contained JSX module string. The output
mirrors the `@mdx-js/mdx` public contract: a default-exported
`MDXContent({components}) → JSX` function. By emitting JSX *source* (rather
than an ESTree), the output feeds through the existing SWC pipeline in
`crates/zfb-render`, keeping JSX → JS codegen in one place.

```rust,ignore
use zfb_content::{compile_mdx_to_jsx_module, MdxJsxOptions, MdxModuleCache};

let cache = MdxModuleCache::default();
let compiled: CompiledMdx = compile_mdx_to_jsx_module(
    source,
    &MdxJsxOptions::default(),
    &cache,
)?;
```

### 3. Frontmatter extraction (`frontmatter`, `tsx_frontmatter`)

`frontmatter::extract` dispatches by extension:

- `.md` / `.mdx` → parse YAML delimited by `---` markers, convert to `serde_json::Value`.
- `.tsx` → statically extract `export const frontmatter = { /* literal-only */ }` via `tsx_frontmatter::extract`.

The public surface is JSON-only; `serde_yaml::Value` is an implementation detail.

Sibling literal exports `extension` and `contentType` in `.tsx` files are also
captured and used by the engine when computing output filenames and response types.

### 4. Content collections (`collection`, `content_bridge`, `schema`)

`collection` mirrors Astro-style content collections: a typed directory of
markdown/MDX/TSX entries walking to validated `Entry<T: Validate>` values.
`walk_collection` is the single source of truth for slug derivation,
`rel_path` computation, frontmatter parsing, and `module_specifier` generation
(`mdx://` or `tsx://` scheme).

`content_bridge` defines the serializable Rust → JS bridge contract so TSX
page modules can call `getCollection("docs")` synchronously during SSR:

```rust,ignore
use zfb_content::{build_snapshot, ContentSnapshot, CollectionConfig};

let snapshot: ContentSnapshot = build_snapshot(&configs)?;
// snapshot.collections: BTreeMap<String, Vec<EntrySnapshot>>
```

`build_snapshot` is deterministic: collections are keyed in a `BTreeMap`
(sorted by collection name) and each `Vec<EntrySnapshot>` is sorted by slug.
Two calls with identical input always produce byte-identical `serde_json`
output, which the build cache relies on.

`schema` implements a minimal JSON-Schema subset used to (a) generate
`.zfb/types.d.ts` TypeScript declarations and (b) validate frontmatter at
`zfb check` time.

### Syntax highlighting (`syntect_highlight`)

Provides a `Highlighter` cache around syntect's `SyntaxSet` and `ThemeSet`.
`Highlighter::highlight` returns an HTML fragment:
`<pre class="syntect-{theme-slug}"><code>…spans…</code></pre>`.
The per-`<pre>` class allows CSS theming while syntect-coloured spans are
preserved inside.

## Key public types

| Type | Module | Role |
|---|---|---|
| `build_snapshot` | `content_bridge` | Build the JS-bridge snapshot |
| `ContentSnapshot` | `content_bridge` | `BTreeMap`-keyed collection map |
| `EntrySnapshot` | `content_bridge` | Serializable entry (slug, frontmatter, body, specifier) |
| `CollectionConfig` | `collection` | Per-collection directory + schema config |
| `compile_mdx_to_jsx_module` | `mdx_jsx_emit` | MDX → JSX module string |
| `CompiledMdx` | `mdx_jsx_emit` | Output of the MDX emitter |
| `MdxJsxOptions` | `mdx_jsx_emit` | Options for the JSX emitter |
| `MdxModuleCache` | `mdx_jsx_emit` | Compilation cache (reuse across files) |
| `MdxModuleSpecifier` | `mdx_jsx_emit` | `mdx://` URL scheme |
| `UnifiedFrontmatter` | `frontmatter` | Parsed frontmatter as JSON |
| `FrontmatterError` | `frontmatter` | Frontmatter parse failure |
| `TsxFrontmatter` | `tsx_frontmatter` | Static extraction result for `.tsx` files |
| `ExternalLinksPlugin` | `plugins` | Hast plugin: open external links in new tab |
| `TocConfig` | `plugins` | Table-of-contents plugin config |
| `PipelineSpec` | `pipeline_spec` | Full pipeline configuration (single knob list shared by snapshot + bundler walks) |
| `BridgeError` | `content_bridge` | Error type for snapshot building |
| `MarkdownFeaturesConfig` | re-export (zfb-md-extras) | Feature toggles for the markdown pipeline |
| `FeatureToggle` | re-export (zfb-md-extras) | Enable/disable individual features |
| `DirectiveSpec` | re-export (zfb-md-extras) | Custom directive configuration |

## Tests

```sh
cargo test -p zfb-content
```
