# Cargo internal-rewire inputs against integrated main `e1ef4f5b`

Proposed `--reason cargo-internal-rewire` inputs for this candidate's lock, recorded 2026-10-17 on the owner's native macOS arm64 host (Darwin 25.6.0). **This is not approval:** none of it has been reviewed as an admission. No contract, pin, ceiling, or allowance moved, and no `--stage` or `--apply` ran.

| Input | Value |
| --- | --- |
| Old lock (what both live contracts pin) | `e1ef4f5b59a2bba260eac29ee8edfdfbcd928335:Cargo.lock`, `13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37` |
| New lock (candidate) | `230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138` |
| Delta | added `zfb-js-host@0.0.0`; changed `zfb-render@0.0.0` (`sha1`, `sha2` → `zfb-js-host`) |
| Manifests | `crates/zfb-js-host/Cargo.toml` and `crates/zfb-render/Cargo.toml` both carry `publish = false`, as committed |

`declaration.json` was derived mechanically from the two locks with the repository's own `parseCargoLock` / `parseLockRecord`. It supersedes [`../cargo-rewire/declaration.json`](../cargo-rewire/declaration.json) for this candidate. That file binds the pre-#318 pair `2726cdb8…` → `d0f3e676…` and is kept as history.

The four `closure-*.json` receipts come from `scripts/record-cargo-producer-closure.mjs --producer zfb`. The old receipts ran in a clean checkout of `e1ef4f5b`, the new ones in this candidate, under `cargo 1.93.1` (rustup `stable` on this host, not CI's 1.99.0). They hold **`cargo metadata --filter-platform` target metadata, not native execution**. The `aarch64-apple-darwin` receipt is not a Mac measurement. The `x86_64-unknown-linux-gnu` receipt was not produced on Linux.

| Target | Old rows | New rows | Only delta |
| --- | ---: | ---: | --- |
| `x86_64-unknown-linux-gnu` | 497 | 498 | `zfb-js-host` |
| `aarch64-apple-darwin` | 481 | 482 | `zfb-js-host` |

The rows are byte-identical to the earlier `../main-sync-darwin-2026-10-16/` and `../refresh-linux-2026-10-16/` receipts.

`proveCargoInternalRewire`, called through `readCargoRewireEvidence` with `--old-cargo-ref e1ef4f5b…:Cargo.lock`, accepted both targets. Two mutations were refused:

- an emptied `addedDependencies`;
- an invented external feature in a closure row.

That proof covers lock and closure structure only. Admission still needs fresh native artifacts for this exact source on both platforms, a reviewed paired `--stage`/`--apply`, and a final enforced success.
