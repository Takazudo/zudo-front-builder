# Live activation checklist and next runtime milestone

This file records what has to happen, in order, for the island-size contracts to
admit this branch's `Cargo.lock` through `--reason cargo-internal-rewire`
(#4258), and what the next runtime milestone is once phase one is accepted. It
authorizes nothing: the tooling living in draft PR [#4259-tooling placeholder]
is a capability, the dispatches below are separately authorized, and the
numbers here are inputs, not results.

## 1. Exact identities

| Input | Value |
| --- | --- |
| Old `Cargo.lock` (both live contracts pin it) | `2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e`, stored at `main` `6fb3fbe0103c5c90799c3262ca5c1ab9ade0f27e:Cargo.lock` |
| New `Cargo.lock` (this branch) | `d0f3e6764ef9233e74ea89a931f9ebfcd6b05f429db36e5ce65f2ddd5685b1bb`, stored at `4fb3224dd5aea3ffac68fba55722aaf7f691b48c:Cargo.lock` and every later commit of `prototype/deno-core-exit` |
| Declared transition | `prototypes/direct-v8/evidence/cargo-rewire/declaration.json`: `zfb-js-host@0.0.0` added (record `db19fe3f…`), `zfb-render@0.0.0` rewired (`fe8968d7…` → `ac163abe…`, removes `sha1`, `sha2`, adds `zfb-js-host`) |
| Producer closure receipts | `prototypes/direct-v8/evidence/cargo-rewire/closure-{old,new}-{x86_64-unknown-linux-gnu,aarch64-apple-darwin}.json` (496→497 and 480→481 rows; one internal row joins, no external drift) |
| Internal manifests | `crates/zfb-js-host/Cargo.toml` and `crates/zfb-render/Cargo.toml`, both `publish = false`, workspace members under `crates/*` |
| Live contracts (unchanged) | `research/v3-island-size/decision-linux-x64.json` and `decision.json` at v4.5.0, cargo pin `2726cdb8…`, fingerprint `e0c64728…`, policy 1 |

The old lock is read only through `git show <commit>:Cargo.lock`; the new lock
is the checkout's own file. If this branch is rebased and its lock bytes move,
the declaration and all four receipts must be regenerated and re-reviewed.

## 2. Order of operations

1. **Land the tooling.** Draft PR `policy/cargo-internal-rewire` is stacked on
   `base/post-4-5-0-island-maintenance` (#4234, held). It reaches `main` only
   through #4234's own authorized integration, then its own. Until then the
   reason does not exist in the checkout that the CI measurement runs from.
2. **Rebase/refresh this branch** onto the `main` that contains the tooling,
   so the measured source carries both the rewired lock and the CLI that can
   admit it. Re-check that `Cargo.lock` is still `d0f3e676…`; otherwise
   regenerate the declaration and receipts.
3. **Collect native evidence on both platforms, from that exact source.**
   - Linux x64: the `health` job's island-size step on the PR (workflow
     `health.yml`; artifact `island-size-linux-x64`, retention 7 days). The
     enforced `check-budget.mjs` step is expected to **fail exclusively** on
     the old Cargo pin (`local Cargo.lock: SHA-256 differs from the reviewed
     contract`, `workspace/packed provenance: Cargo lockfile differs from the
     contract`). Every other line must be clean; raw and `gzipNormalized`
     totals must equal the contract. Keep that run and its log; it is retained
     input, not a pass.
   - Darwin arm64: `island-size-darwin.yml` via `workflow_dispatch` with
     `ref=<pushed head>`, `platform=darwin-arm64` (the default) or `both`, `enforce=true`,
     `css_carriers=false`; artifact `island-size-darwin-arm64-<source sha>`.
     The same exclusive-old-pin rejection is the expected initial result.
   - A missing artifact, any moved total, or any additional failure line is a
     blocker, not something to re-pin over.
4. **Record the four closure receipts at the measured source** with
   `scripts/record-cargo-producer-closure.mjs` (old at `main`'s lock, new at
   the candidate's) and confirm they are byte-identical to the committed ones
   or replace them, re-reviewing the declaration.
5. **Stage both platforms into one directory** from the same checkout:

   ```sh
   node scripts/rebaseline-island-size-linux.mjs --artifact <island-size-linux-x64 dir> \
     --run-id <run> --job-id <job> --head-sha <PR head> --source-sha <merge source> \
     --reason cargo-internal-rewire --expected-old-cargo-sha 2726cdb8…678e \
     --expected-new-cargo-sha d0f3e676…b1bb --old-cargo-ref 6fb3fbe0103c5c90799c3262ca5c1ab9ade0f27e \
     --new-cargo-ref <commit storing the new lock> \
     --rewire-declaration prototypes/direct-v8/evidence/cargo-rewire/declaration.json \
     --old-closure prototypes/direct-v8/evidence/cargo-rewire/closure-old-x86_64-unknown-linux-gnu.json \
     --new-closure prototypes/direct-v8/evidence/cargo-rewire/closure-new-x86_64-unknown-linux-gnu.json \
     --stage <dir>
   node scripts/rebaseline-island-size-darwin.mjs --artifact <island-size-darwin-arm64 dir> \
     --run-id <run> --job-id <job> --artifact-id <id> --artifact-expires-at <ISO> \
     --source-sha <measured source> --reason cargo-internal-rewire \
     --expected-old-cargo-sha 2726cdb8…678e --expected-new-cargo-sha d0f3e676…b1bb \
     --old-cargo-ref 6fb3fbe0103c5c90799c3262ca5c1ab9ade0f27e --new-cargo-ref <commit> \
     --rewire-declaration prototypes/direct-v8/evidence/cargo-rewire/declaration.json \
     --old-closure prototypes/direct-v8/evidence/cargo-rewire/closure-old-aarch64-apple-darwin.json \
     --new-closure prototypes/direct-v8/evidence/cargo-rewire/closure-new-aarch64-apple-darwin.json \
     --stage <dir>
   ```

   Both runs must record the same `rewire` identity (declaration digest, old
   and new lock) and transition; `--source-sha` on Darwin must be the Linux
   merge source or its probe head.
6. **Review the stage directory** (contract diffs move only
   `toolchain.cargoLockSha256`, `outcome`, `baselineSourceSha`,
   `platformStatus`, `baselineEvidence`; README and budget-test pins), then
   `node scripts/rebaseline-island-size-linux.mjs --apply <dir>` from the same
   checkout, in one commit.
7. **Final enforced verification.** Push, and obtain a green `health` run
   (island-size step passing, no provenance line) and a green enforced
   `island-size-darwin.yml` run at the applied head. Only then is the
   producer transition admitted and #4242's previously skipped Health steps
   can be reconciled at that head.

Nothing in this session performed steps 1–7 beyond preparing the inputs and
proving, with the tooling in its worktree, that the declaration, locks,
manifests and receipts are admissible at the lock level for both targets.

## 3. Next runtime milestone (recorded, not executed)

After phase one is accepted and the producer transition is admitted:

1. **Actual CLI typed-config path.** Load `zfb.config.ts` through the existing
   `zfb-config-loader` esbuild preparation into the direct-V8 host behind an
   internal build-time opt-in (a non-default Cargo feature on `zfb`, no public
   backend selector), keeping `parse_loaded_config` defaults, presets and
   field-path diagnostics unchanged (`check-outer-config.py` is the oracle).
2. **Render factory / threaded adapter.** Implement `EmbeddedV8HostFactory` and
   `ThreadedV8Host` on the direct host with owned DTOs across the pinned
   thread, one process initializer, deterministic ICU preparation, and prove
   the candidate CLI's selected graph excludes Deno runtime crates
   (`record-cargo-producer-closure.mjs` on the opt-in feature).
3. **Dev lifecycle.** Start-before-swap, failed-replacement rollback and finite
   shutdown through the existing dev supervision tests, plus reload under the
   direct host. Standalone-host evidence from this branch does not prove any
   of this.

Five-target packaging/native adoption, Rolldown interoperability (#4120 stays
independent), default switching and final Deno deletion remain later,
separately authorized work.
