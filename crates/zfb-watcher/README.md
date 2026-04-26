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
- **Drop = stop.** Dropping the `Watcher` handle aborts the debouncer
  task and drops the underlying notify watcher (which stops the OS-level
  watch).

## Non-goals

- **Polling for re-appearance** of a top-level missing path is *not*
  implemented. Sub-directories appearing under an already-watched parent
  are picked up automatically by recursive watching; if `data/` itself
  is created later the dev server is expected to restart the watcher.
- **No "watch a single file via its parent dir" trickery** — we simply
  call `notify::Watcher::watch` on whatever path is given. `notify`
  handles the file-vs-dir distinction.
