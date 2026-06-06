# zfb-test-utils

Shared test helpers for `zfb` integration tests (std-only, no runtime dependencies).

This crate consolidates utilities used across multiple integration test suites in the
workspace: esbuild binary location, the compiled `zfb` binary path, and HTML normalization
for snapshot comparison.

## Public API

### `locate_esbuild() -> Option<PathBuf>`

Finds an esbuild binary suitable for integration tests. Resolution order:

1. `ZFB_ESBUILD_BIN` env var — if set and points to an existing file, return immediately.
2. Workspace-local slot: `<workspace_root>/crates/zfb/binaries/esbuild/<binary>`.
3. pnpm nested store: `node_modules/.pnpm/*/node_modules/@esbuild/<suffix>/bin/<binary>`.
4. pnpm flat store: `node_modules/.pnpm/node_modules/esbuild/bin/<binary>`.
5. Portable `PATH` walk via `std::env::split_paths` (no `which` shell-out — absent on Windows).

Returns `None` if no candidate is found. Callers should gate with:

```rust,ignore
let Some(esbuild) = zfb_test_utils::locate_esbuild() else { return; };
```

### `zfb_binary!()` macro

```rust,ignore
use zfb_test_utils::zfb_binary;
let zfb = zfb_binary!();  // PathBuf
```

Expands to `PathBuf::from(env!("CARGO_BIN_EXE_zfb"))` at the call site. The macro form
is required because `CARGO_BIN_EXE_zfb` is set by Cargo only when compiling integration
tests in the `crates/zfb/` crate — a plain function call cannot access that env var from
another crate's compile unit.

### `normalize_html(html: &str) -> String`

HTML5-spec-compliant normalization for snapshot-based integration tests. Implemented with
`html5ever` + `markup5ever_rcdom`. Produces canonical output:

- **Attributes** sorted lexicographically by local name.
- **Boolean attributes** (`disabled`, `checked`, `readonly`, …) canonicalized to
  empty-string form (`disabled=""`).
- **Void elements** serialized as `<br>` (no `/>`) per HTML5.
- **Entity encoding**: `&amp;`, `&lt;`, `&gt;`, `&quot;`; `&#x27;` / `&apos;` decoded
  to literal `'`.
- **Whitespace**: pure-whitespace text nodes between elements collapsed to `\n`; content
  inside `<pre>`, `<code>`, `<textarea>`, `<script>`, `<style>` preserved verbatim.
- **Idempotent**: `normalize_html(normalize_html(s)) == normalize_html(s)`.

## Tests

```sh
cargo test -p zfb-test-utils
```

Tests in `src/html_normalize.rs` cover attribute ordering, entity encoding, boolean
attribute forms, void elements, whitespace collapse, literal preservation in raw-text
contexts, and idempotency.
