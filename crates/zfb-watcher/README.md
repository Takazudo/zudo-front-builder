# zfb-watcher

Dev-mode filesystem watcher for the zfb dev server.

Wraps the [`notify`](https://crates.io/crates/notify) crate's recommended
watcher, normalises the many event kinds into a small `ChangeKind` enum,
and emits debounced `Change` events on a `tokio::sync::mpsc::Receiver`.

## Public API

```rust,ignore
use zfb_watcher::{Watcher, Change, ChangeKind};

let (handle, mut rx) = Watcher::start(
    "/path/to/project",
    [
        "content",
        "pages",
        "components",
        "layouts",
        "styles",
        "data",
        "public",
        "zfb.config.ts",
    ],
)?;

while let Some(Change { path, kind }) = rx.recv().await {
    match kind {
        ChangeKind::Created  => { /* ... */ }
        ChangeKind::Modified => { /* ... */ }
        ChangeKind::Removed  => { /* ... */ }
    }
}

// Drop `handle` to stop watching.
```

## Behaviour

- **Recursive watching** of every existing relative path passed in.
- **Missing paths are skipped** with a `warn!` log — `data/` not existing
  yet is normal and not an error.
- **~50ms debounce window** by default (override via
  `Watcher::start_with_debounce`). Editor "save" bursts (vim's swap-and-
  rename, vscode's metadata-then-content sequence, etc.) collapse to a
  single `Change` per path per burst.
- **Three-variant `ChangeKind`** (`Created` / `Modified` / `Removed`).
  Anything notify reports that we cannot positively classify falls into
  `Modified` — we'd rather rebuild unnecessarily than miss a real change.
- **Drop = graceful flush-and-detach.** Dropping the `Watcher` handle sends a
  shutdown signal to the debouncer so pending events can flush, then detaches
  the JoinHandle without aborting. The underlying notify watcher is also
  dropped, which stops the OS-level watch. For a fully-awaited flush, prefer
  the async `Watcher::shutdown()` method.

## Git-restore reconciliation

Raw FS events from `git checkout --`, `git pull`, and `git stash pop` can arrive
out of order: a `Remove` event with no paired `Create` (bare-Remove platform
quirk) or a `Remove`/`Create` pair within one debounce window. The debouncer
handles both cases at flush time — a pending `Removed` whose path exists on disk
is upgraded to `Modified`, and a pending `Removed` followed by `Created` takes
the `Created` kind — so downstream rebuild logic always sees the correct change
kind instead of a stale deletion (issue #823).

## Extra watch paths

`Watcher::start_with_extras` accepts a second set of absolute paths watched in
addition to the relative paths joined against `project_root`. Use this for
out-of-tree directories (e.g. a shared design-token package) that must
participate in dev-mode rebuilds without being under the project root.

## Reconciled recursive dir watches

`Watcher::sync_recursive_dir_watches(desired_roots, skip_dir_names)` maintains
a replace-semantics set of recursively watched directory roots (built for CSS
sibling mirror roots — issue #1801) on top of a running watcher:

- Skip-dir names are **caller-supplied** and matched as exact path components
  at any depth, on delivery — so `dist` never suppresses `distress`, and a
  `node_modules/` created after registration is still suppressed. Filtering
  happens in the notify callback, before the debounce pipeline, so a skip-dir
  flood (a cargo build in a watched sibling's `target/`) never reaches the
  channel.
- Each call supplies the full desired set; roots that fall out are unwatched
  (a root that doubles as a `watch_additional_files` dependency parent is
  downgraded back to non-recursive instead, preserving that consumer's
  coverage). Only genuinely new roots are returned.
- Suppression never narrows boot-root or dependency-parent delivery — the
  watcher's output stays a superset of what it delivered before the sync.

## Non-goals

- **Polling for re-appearance** of a top-level missing path is _not_
  implemented. Sub-directories appearing under an already-watched parent
  are picked up automatically by recursive watching; if `data/` itself
  is created later the dev server is expected to restart the watcher.
- **No "watch a single file via its parent dir" trickery** — we simply
  call `notify::Watcher::watch` on whatever path is given. `notify`
  handles the file-vs-dir distinction.
