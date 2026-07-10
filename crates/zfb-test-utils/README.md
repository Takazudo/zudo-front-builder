# zfb-test-utils

Shared test helpers for `zfb` integration tests (std-only, no runtime dependencies).

This crate consolidates utilities used across multiple integration test suites in the
workspace: esbuild binary location, the compiled `zfb` binary path, HTML normalization,
SSE parsing, watcher readiness handshakes, and cross-binary e2e serialization.

## Public API

### `locate_esbuild() -> Option<PathBuf>`

Finds an esbuild binary suitable for integration tests. Resolution order:

1. `ZFB_ESBUILD_BIN` env var — if set and points to an existing file, return immediately.
2. Workspace-local slot: `<workspace_root>/crates/zfb/binaries/esbuild/<binary>`, probed for
   every candidate workspace root. Roots are re-derived **at runtime** (walk up from
   `current_dir()`, then `current_exe()` ancestry, looking for the `Cargo.toml` + `crates/`
   marker); the compile-time `CARGO_MANIFEST_DIR`-derived root is only a fallback. A stale
   rlib reused from another checkout path (shared target dir, issue #1007) therefore cannot
   pin an outdated path and silently skip gated tests.
3. pnpm nested store, per root: `node_modules/.pnpm/*/node_modules/@esbuild/<suffix>/bin/<binary>`.
4. pnpm flat store, per root: `node_modules/.pnpm/node_modules/esbuild/bin/<binary>`.
5. Portable `PATH` walk via `std::env::split_paths` (no `which` shell-out — absent on Windows).

Returns `None` if no candidate is found — a graceful skip; machines without esbuild stay
green. Panics **only** on the harness-bug state: the expected slot binary exists as a file
under a candidate root, yet lookup still failed (the issue #1007 silent-skip incident). The
slot directory being non-empty never triggers the panic (it permanently holds `.gitkeep`
and `README.md`). Callers should gate with:

```rust,ignore
let Some(esbuild) = zfb_test_utils::locate_esbuild() else { return; };
```

The lower-level `candidate_workspace_roots()`, `SkipKind`, and
`classify_failed_lookup()` helpers are public so the lookup policy and its
panic-vs-skip contract can be tested directly.

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

### `CrossBinaryE2eLock`

OS-level advisory lock for heavy V8/esbuild e2e tests that must not boot at
the same time across separate integration-test binaries.

```rust,ignore
let _e2e_lock = zfb_test_utils::CrossBinaryE2eLock::acquire();
```

The lock file lives under `<workspace_root>/target/.e2e-serialize.lock`.
`acquire()` waits up to 360 seconds by default; `acquire_with_timeout(...)`
exists for focused tests. Dropping the guard releases the OS lock, and process
death releases it too.

### SSE helpers

- `wait_for_subscribers(tx, min, dur)` waits for a broadcast channel to have
  enough live receivers using a `Notify`-keyed wake.
- `wait_for_subscribers_polled(tx, min, dur)` is the legacy 10 ms polling
  variant for externally-owned senders.
- `next_sse_event_name(resp, dur)` reads a `text/event-stream` response and
  returns the first non-empty `event:` name, decoding UTF-8 incrementally so
  split multibyte characters at chunk boundaries do not drop already-seen
  lines.
- `decode_utf8_incremental(...)` is public for unit tests of that decoder.

### Watcher readiness handshake

`watcher_live_handshake(opts, write_marker, signal_seen)` writes fresh marker
files until a caller-supplied readiness predicate observes the watcher stream
is live. This replaces fixed warmup sleeps in tests that can otherwise race
FSEvents/inotify startup dead windows.

`HandshakeOpts` controls the deadline and write/poll cadence. `HandshakeResult`
reports whether the stream became live, how many markers were written, and the
elapsed time.

## Tests

```sh
cargo test -p zfb-test-utils
```

Tests in `src/html_normalize.rs` cover attribute ordering, entity encoding, boolean
attribute forms, void elements, whitespace collapse, literal preservation in raw-text
contexts, and idempotency.

Additional unit tests cover esbuild lookup classification, cross-binary lock
mutual exclusion, SSE UTF-8 chunk decoding, and watcher-handshake timing.
