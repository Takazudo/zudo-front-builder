# YAML differential evaluation #4049

Prepared for `verify/sweep-261008-yaml`, based on captured main
`cb4b6dbd3af21980b302916c5d97d4f767c51574`. This implements the confirmed
four-scope plan and `CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`:
independent preparation; manager owns serialized verification and integration.
Only this research directory ships. No dependency bump, probe merge, release,
watcher baseline refresh, or verdict is supplied by preparation.

The owner explicitly starts this evaluation now, superseding the old timing hold.
The complete protocol in [#2988](https://github.com/Takazudo/zudo-front-builder/issues/2988)
and latest KEEP in `DEPENDENCIES.md` (0.0.51, #3105) were read. Prior KEEP remains
authoritative for 0.0.51, not a verdict for this candidate. The previous candidate
and its same-toolchain baseline both failed the render ceiling. Nothing here
relaxes the unchanged size gate.

## Frozen candidate

Registry and actual downloaded archives establish **0.0.55**, the newest complete
non-yanked released pair at preparation. The older 0.0.54 alert is superseded.
`provenance.json` contains registry timestamps, checksums, license/MSRV and the
normalized published manifests; `github-provenance.json` records annotated tags,
dereferenced commits, Releases/checksums and zero open pull requests on both repos.
These are start observations, not final race evidence.

| Crate | SHA-256 | Archive bytes |
| --- | --- | --- |
| noyalib | `0c3ccce0306ea4477a655985ff2bf3e6c38f78f3e1defef6f30c7eb24d58cf41` | 1,274,912 |
| noyalib-serde-yaml | `4775f0afd3f1d5e55b3779e8e4b7ffde09d690629f6731e4047159a751c17a3b` | 72,952 |

Both declare `MIT OR Apache-2.0`, minimum Rust `1.86.0`. The published alias has
`build = false`, forbids unsafe and re-exports only
`noyalib::compat::serde_yaml::*`; sole normal dependency exact `=0.0.55`, no default
features, `std` + `compat-serde-yaml`. Archives were read in memory, never executed
or extracted to a target. Its Release predicts duplicate keys are now refused
for `Value`, conflicting with our immutable last-wins case. Test that claim;
never rewrite the expectation to fit it. Upstream tests are predictions only.

The published core contract was also read: custom-tag Display still predicts
column 8 with `(line, column, index) = (1, 8, 7)`; its duplicate-key test still
predicts last-wins. That conflicts with the 0.0.55 Release wording. Neither
upstream statement replaces the unchanged local harness result.

## Ordered manager commands

Use one task-local Cargo home/toolchain and one shared native target chosen by
the manager. Do not install a second compiler or create child targets. Read and
inspect the supported installer before running it. Pin the active toolchain's
bin directory for every native and wasm command; `rust-toolchain.toml` selects
stable, which must resolve to one captured version for both pins.

1. Capture host/tool versions, initial disk bytes and peak RSS/disk sampling in
   the evidence directory. Supply frozen pnpm dependencies with
   `pnpm install --frozen-lockfile`; record its exit and unchanged lockfile.
   Preflight `rustc -Vv`, `cargo -V`, `rustup show active-toolchain`,
   `rustup target list --installed`, `wasm-bindgen --version` (exact 0.2.121),
   package binaryen's `wasm-opt --version` (130), `node --version`,
   `pnpm --version`, `cargo deny --version` and active executable paths.
   A missing tool is an unmet check. Manager installs missing bindgen only by
   the documented throwaway `cargo install --root ... --locked --version 0.2.121`
   route, guarded; record/remove that owned root after use.

2. Run Phase 1 under the machine-wide guard, from this checkout:

   ```sh
   export YAML_EVIDENCE=/workspace/scratch/zfb-sweep-261008/yaml-evidence
   export CARGO_HOME=/workspace/scratch/zfb-sweep-261008/cargo
   export RUSTUP_HOME=/workspace/scratch/zfb-sweep-261008/rustup
   export CARGO_TARGET_DIR=/workspace/scratch/zfb-sweep-261008/rust-target
   # Manager prepends the captured toolchain bin directory to PATH first.
   bash "$HOME/.codex/scripts/heavy-guard.sh" -- bash research/4049-yaml-evaluation/phase1.sh
   ```

   This saves protected hashes, baseline manifest/lock/size manifest, reads both
   baseline graphs, refreshes registry provenance, pins only the
   package alias, resolves and verifies BOTH lock checksums **before compiling**,
   and runs the unchanged harness plus protected tests and separate checks.
   Every command's full output, exit and argv are saved outside the checkout.
   Run `audit.sh` separately at both pins with `YAML_PIN_LABEL=baseline` or
   `candidate`, under the guard and while that pin is active. It records all
   graph/license commands independently; a missing cargo-deny is NOT RUN and
   blocks the overall gate without hiding available semantic evidence.
   An EXIT trap restores manifests and verifies all protected hashes even on
   failure. Never reuse an evidence directory: the snapshot refuses overwrite.
   The script stops on failed checks; report later checks as NOT RUN.

3. If P1 is not 18/18, inspect all 18 observations and protected diagnostics.
   P3 is conditional: the alias is the same direct compat implementation, so
   document whether an independent direct shim can change the failure at all.
   A P3 experiment that needs production edits exceeds this preparation scope;
   obtain the manager's bounded execution plan before changing source. Do not
   convert predicted equality into a tested result. Apply the standing abandon
   rule: >40 formatted non-test production lines, a second corrective round,
   unsafe/FFI/OS branches/polling/signal supervision/Unicode tables ⇒ stop,
   restore, KEEP. A missing execution environment yields BLOCKED, not KEEP.

4. Only after 18/18, protected assertions, separate native checks, dependency
   audit and abandon-rule review pass, prepare Phase 2. Re-pin with
   `python3 research/4049-yaml-evaluation/evaluate.py pin --evidence "$YAML_EVIDENCE"`,
   copy the saved candidate lock back, and rerun `... evaluate.py lock ...`.
   Keep a manager shell EXIT trap executing `restore` then `verify` throughout.
   Immediately before Phase 2 run:

   ```sh
   python3 research/4049-yaml-evaluation/evaluate.py disk --evidence "$YAML_EVIDENCE"
   ```

   The exact unchanged minimum is **32,212,254,720 free bytes (30 GiB)**,
   not df's rounded display. Record each command through a pipefail logger and
   its actual child exit, including failed builds; guard all Rust/build runs.

5. Candidate Phase 2, in sequence:

   ```sh
   cargo check --locked --target wasm32-unknown-unknown -p zfb-md-wasm
   pnpm test:md-wasm
   CARGO_TARGET_DIR="$YAML_EVIDENCE/wasm-candidate" node scripts/run-zfb-md-wasm-build-timed.mjs
   node scripts/assert-zfb-md-wasm-budgets.mjs --build-log "$YAML_EVIDENCE/candidate-build.log" --dist crates/zfb-md-wasm/npm/dist --update-manifest
   ```

   The timed target must initially be absent. Preserve the timed build's output
   as `candidate-build.log`. The budget script's temporary manifest change is
   measurement only and must be restored even after failure. If the strict
   composite build fails, preserve FAIL; a separately run
   `pnpm --filter @takazudo/zfb-md-wasm test` may supply diagnostic evidence only.
   Do not use `test:md-wasm:local`, `--allow-over-ceiling`, altered ceilings,
   a projected CI delta, or a direct test-half result to declare Phase 2 green.

6. Save all candidate artifacts/metrics **before** building control (production
   builds overwrite generated outputs), then restore original 0.0.44 manifests
   and size manifest with `... evaluate.py restore ...`. Recheck tool identities
   and disk; build the control on that identical toolchain:

   ```sh
   CARGO_TARGET_DIR="$YAML_EVIDENCE/wasm-control" node scripts/run-zfb-md-wasm-build-timed.mjs
   node scripts/assert-zfb-md-wasm-budgets.mjs --build-log "$YAML_EVIDENCE/control-build.log" --dist crates/zfb-md-wasm/npm/dist --update-manifest
   ```

   Both targets initially absent; record sixteen fields, complete dist bytes,
   ceilings/headroom/deltas, CPU/OS, total wall time, peak RSS/disk and all twelve
   production subprocess times per successful timed run. The 210-second Apple-M4
   reference is informational on this host; failure at a budget check may prevent
   the timed wrapper's final summary, so record that as unavailable. Tarball
   3,900,000 B ceiling remains unchanged; if pack evidence is required, pack from
   the emitted dist to the scratch directory and measure it, never publish.

7. Compare baseline/candidate normal + feature closure, exact two-entry lock
   delta and unchanged package count (measure, do not assume historic 597), both
   `cargo deny list` category counts (normalize only YAML version strings) and
   both `cargo deny check` results. No new exception. Re-pin only if needed for
   missing candidate evidence, then restore again. Hash restoration must pass;
   revert any tracked generated outputs from the known clean initial revision
   only after inspecting their diff. Never blanket-delete targets or other work.

8. Immediately before any final verdict run `... evaluate.py registry ...` again,
   saving stdout **and exit** to final evidence; it rechecks adopted/candidate
   yanks, actual archives, alias exact dependency and newer releases. Through the
   connected GitHub fetch tool repeat both tag refs → annotated objects → commits,
   Releases with checksum bodies and open pull collections (paginate if needed),
   comparing `github-provenance.json`. Re-run the existing drift detector with
   authenticated read access; preserve all deltas/errors/exit (10 is drift, not
   test failure). Never redirect a snapshot onto the committed watcher baseline.
   Record release-race row (a), both registry timestamps/yanks for each skipped
   0.0.45–0.0.54 release, newer half-pairs, open future release PRs, and disposition
   of any newly complete pair. Newer complete pair means reselect/re-evaluate,
   not silently bless stale 0.0.55 evidence. Public gh access is forbidden in this
   environment; the read-only GitHub connector succeeded for initial provenance.

9. Run restoration/hash verification, inspect `git status` and allowed scope.
   Fill `report.md` with observed counts, exact source SHA, commands/logs/status,
   all size columns, blockers, final race and next trigger. Only complete evidence
   permits KEEP/MIGRATE; MIGRATE additionally requires every Phase 2 gate green.
   Baseline over budget still cannot permit MIGRATE. Manager alone performs
   external comments/integration/CI; this topic commits locally only.

## Feasibility and remaining work

Preparation ran no Rust, build, browser or installation. Cold native work can
take tens of minutes; prior targets used 8.3–12 GiB, plus two cold wasm targets
and toolchain/dependency storage. The current filesystem is 32 GiB total, about
30 GiB free **before** manager installation; the exact Phase 2 gate is likely
blocked afterwards. No deletion/reconfiguration or reduced gate is authorized.
Manager can run Phase 1 as resources permit and retain a disk blocker for Phase 2.
The prior successful timed comparison duration does not estimate a cold host.

Cheap preparation verification: Python syntax, Bash syntax, JSON parsing,
immutable 18-case inventory and fail-closed lock-check negative check. These
establish control-script behavior, never YAML compatibility. Manager separately
ran the unchanged 0.0.44 harness: 4/4 (including the 18-case equality assertion),
guard PASS in 64 seconds; log at
`/workspace/scratch/zfb-sweep-261008/yaml-baseline-harness.log`. That baseline
evidence proves no candidate result.

Manager targeted candidate evidence: the unchanged 0.0.55 harness passed 4/4,
including all 18 observations, guard PASS in 52 seconds. Logs:
`/workspace/scratch/zfb-sweep-261008/yaml-candidate.log` and
`/workspace/scratch/zfb-sweep-261008/yaml-candidate/harness.log`. This resolves
the Release wording contradiction for the consumed Value path. Full Phase 1
and audit remain pending; current disk is below 30 GiB after install, so
Phase 2 is blocked. Temporary Cargo files and protected hashes were restored.
