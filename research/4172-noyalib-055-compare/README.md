# noyalib 0.0.53 → 0.0.55 comparison #4172

This harness compares the exact published noyalib / noyalib-serde-yaml **0.0.55** pair
with the shipping **0.0.53** pair. It covers the real frontmatter path
(`zfb_content::frontmatter::extract` → `serde_yaml::from_str::<serde_json::Value>`) and the
md-wasm artifact budgets. It never edits the branch's `Cargo.toml` / `Cargo.lock` and never
applies the production pin. #4173 owns production application. Policy:
`CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`. The manager runs
every compiling step. 0.0.54, 0.0.56 and 0.0.57 are out of scope.

| File | Role |
| --- | --- |
| `compare.sh` | Manager-only driver: scratch worktree, both lanes, lock check, sizes, summary |
| `evaluate.py` | Non-compiling controls: registry, pin, lock delta, flow timing, sizes, decision |
| `probe.rs` | Copied into the scratch copy only, as `crates/zfb-content/examples/yaml_4172_probe.rs` |
| `provenance.json` | Frozen pair facts: checksums, tags, manifests, source changes, lock delta |
| `test_evaluate.py` | Offline unit tests for `evaluate.py` |
| `README.md` → [Report template](#report-template) | Durable `report.md` to fill from `result.json` |
| `result.schema.json` | Shape of `result.json` written by `evaluate.py summarize` |

## Pre-run findings (no compile)

- **Registry:** both pairs are non-yanked. Recomputed archive SHA-256 values match crates.io and
  the GitHub Release checksums. Tags `v0.0.55` dereference to the packaged
  `.cargo_vcs_info.json` commits. License `MIT OR Apache-2.0` and MSRV 1.86.0 are unchanged.
- **Manifests:** the adapter pins `noyalib = "=0.0.55"` (no default features; `std` +
  `compat-serde-yaml`) and changes no code. The core's normal dependencies, features and
  `build.rs` are identical. Its only manifest delta is a dev-only `jsonschema` dependency.
- **Compat config:** `ParserConfig::serde_yaml_compat` and the `SerdeYaml` limits are identical.
- **Lock delta:** a throwaway resolution changed exactly the two pair entries (594 packages,
  dependency lists unchanged). `evaluate.py lock` reported no errors.
- **Duplicate keys:** 0.0.55 refuses duplicate keys only for a `noyalib::Value` target.
  zfb's `serde_json::Value` target keeps last-wins, so the immutable `duplicate-keys` case
  should hold.
- **Implicit flow keys:** keys over 1024 characters are refused, but zfb already refuses
  every non-string key, so only error text can differ.
- **Possible behavior changes:** verbatim tags and alias depth (now charged against
  `max_depth`) can change values or acceptance. The probe observes both.
- **Long-line fix:** the scanner's plain-scalar line lookahead and token-queue compaction were
  quadratic. Upstream reports 400 KB taking seconds and 1.6 MB taking about a minute before
  any budget applied. Both fixes sit on the `from_str` path zfb uses.
- **Toolchain:** the shared target's native artifacts come from the `cargo` on `PATH`
  (Homebrew `rustc 1.94.0` on the manager Mac), while `build.mjs` builds the WASM with
  rustup's active toolchain (`stable`, 1.96.0). `compare.sh` keeps that split to reuse the
  cache. It records both identities per lane, and `summarize` requires them to match across
  lanes.
- **Diagnostics step:** zfb's `diagnostics::tests` is opt-in (`WITH_ZFB_DIAGNOSTICS=1`). It
  needs a no-V8 build of the whole `zfb` crate, and its `build.rs` downloads esbuild. The
  harness already pins the `Display` and `location()` contract that the step consumes.

## Run (manager)

Run from the root checkout or this worktree, on a host with the wasm32 target, wasm-bindgen
0.2.121, and at least 20 GiB free:

```sh
bash research/4172-noyalib-055-compare/compare.sh 2>&1 \
  | tee "$HOME/.cache/zfb-4172-compare-$(date -u +%Y%m%dT%H%M%SZ).log"
```

It refuses `CI`, refuses to reuse an evidence directory, and stops at preflight on a missing
tool or on low disk. Heavy steps queue through `$HOME/.claude/scripts/heavy-guard.sh` (default
`--max-run` 5400 s each). Run it with `run_in_background: true`, and do not start other Cargo
work against the same target while it runs.

Expected cost on the manager Mac:

- **Disk:** 5–10 GB on top of the shared target. Candidate-lane workspace crates get new
  metadata hashes, and the scratch `node_modules` is extra.
- **Time:** about 1.5–3 h. Each lane runs native tests (10–30 min), a four-artifact
  `wasm-release` build (15–25 min) and the flow probe (worst case about 25 min when 0.0.53
  hits the 120 s limits).

The run leaves `result.json`, `decision.txt`, per-step `.command/.log/.exit/.seconds` files,
both lock files, `dist/` copies and tarballs under `$HOME/.cache/zfb-4172-yaml-evidence/<stamp>/`.

To re-summarize without rebuilding (for example once the CI log below arrives):

```sh
CI_CANDIDATE_BUDGET_LOG=/path/ci-candidate.log \
CI_CANDIDATE_RUN='https://github.com/Takazudo/zudo-front-builder/actions/runs/<id> @ <sha>' \
DISPOSITIONS=/path/dispositions.json \
  bash research/4172-noyalib-055-compare/compare.sh summarize "$HOME/.cache/zfb-4172-yaml-evidence/<stamp>"
```

## CI measurement for the hard ceilings

Mac bytes are not CI bytes (`crates/zfb-md-wasm/LOCAL-VS-CI-SIZES.md`; its 2026-09-19 Mac
build put render-only about 13 KB over its ceiling while CI fit). Local Mac sizes are therefore
**same-toolchain relative evidence only**: the per-artifact delta plus the CI 0.0.53 row gives
a projection, never a verdict. The hard-ceiling verdict needs the `wasm-md (default)` job's
budget step at the candidate pin:

1. From the root checkout, create a never-merged scratch branch off the evaluated base. Its
   only commit changes the root pin to `=0.0.55` and runs
   `cargo update -p noyalib-serde-yaml --precise 0.0.55`, giving the same two-entry lock
   delta. Push it, then dispatch `gh workflow run health.yml --ref <scratch-branch>`.
2. Confirm that `wasm-md (default)` actually ran rather than being skipped by `changed-files`.
   The budget step **will fail with `manifest-mismatch`**, because `shipped-sizes.json` holds
   the 0.0.53 bytes. That failure is expected; the step prints every
   `<artifact>: cdylib=… gzip-9=…` line before it throws. Never edit the manifest or ceilings
   to turn it green. The pack step does not run after it, so the CI tarball stays
   unmeasured; the host tarball bytes are reported instead.
3. Save that job's log (`gh run view <run> --job <job> --log > ci-candidate.log`), then
   re-summarize with `CI_CANDIDATE_BUDGET_LOG` and `CI_CANDIDATE_RUN`. The same run's
   `health` job also gives Linux evidence for the harness at the candidate pin.
4. Delete the scratch branch after the log is saved.

## Decision rules

| Outcome | Requires |
| --- | --- |
| **MIGRATE-ELIGIBLE** | Both lanes complete. The candidate passes every lane step the control passes: 18-observation harness, resource budgets, content/API/AST, probe, flow semantics, wasm build/test/exports/pack/release. Every probe diff has a recorded disposition. CI candidate gzip-9 is under every unchanged ceiling, and the tarball is under 3,900,000 B |
| **KEEP 0.0.53** | A *completed* evaluation fails a substantive gate: a candidate-only red step, a recorded frontmatter regression, a candidate long-flow failure inside the limits, or a CI or host ceiling breach where control fits |
| **HOLD** | Any missing, interrupted, ENV_SUSPECT, or not-run step; missing disk, platform, tool or access; a red control lane; mismatched toolchains; no CI candidate measurement; an undispositioned probe diff |

- **Owner exception.** For the exact 0.0.55 pair only, the owner waived the 1,024 B
  discretionary growth threshold and the zero-render-growth criterion, provided behavior checks
  and all unchanged hard ceilings pass (#4161, 2026-10-12). That growth is a note, not a
  failure. It is no ceiling raise and no permission for 0.0.56/0.0.57.
- **Render baseline.** Current CI render-only is 1,094,209 B gzip-9 against 1,100,000 B
  (5,791 B headroom). The recorded Mac 0.0.53→0.0.55 projection of +1,043 B is **not fresh
  evidence**. JS island normalization does not change this WASM budget.
- **Probe diffs.** A documented boundary diff where both pins refuse is error-text-only and
  needs no disposition. Any value change, new refusal or new acceptance, and every
  undocumented diff, needs an entry in a dispositions file:
  `{"<case>": {"disposition": "accepted"|"regression", "reason": "…"}}`. A `regression`
  disposition means KEEP. Never rewrite the immutable corpus or its baseline to fit the
  candidate. A semantic boundary worth keeping becomes an ordinary deterministic test in #4173.
- **Long-flow-line timing.** Timing is reported, never gated: rows, log–log slopes and the
  control/candidate ratio at the largest common size, plus time-to-refusal above `max_nodes`.
  `improvementDemonstrated` is descriptive. If it is false, report the data to the owner,
  whose growth acceptance assumed this fix. Timing thresholds stay out of unit CI.
- **Retries.** A pass on retry is not a pass. Record the failed attempt next to the retry.

## Report template

Fill this from `result.json` (validate it against `result.schema.json`) and commit it as
`report.md` here. Keep raw logs in the evidence directory; quote only exits, sizes and paths.

```md
# noyalib 0.0.53 → 0.0.55 #4172 — <MIGRATE-ELIGIBLE | KEEP | HOLD>

<One or two sentences on the deciding evidence: what is missing (HOLD) or which gate failed (KEEP).>

## Evaluated state

Source SHA, harness SHA-256, evidence path, host, native rustc, WASM rustc, wasm-bindgen,
wasm-opt, node, pnpm, ZFB_RELEASE_VERSION, branch-untouched exit, both lanes' toolchain match.

## Pair provenance

Both pairs' SHA-256 and yanked state from registry.json; the lock-check output (two entries);
newer pairs observed (out of scope).

## Gates

| Step | Control | Candidate |
| --- | --- | --- |
| harness, budgets, error-messages, content, md-api-ast, probe-build, probe-observe, flow, wasm-build, wasm-npm-test, wasm-exports, wasm-pack, wasm-packed, wasm-release | <state> | <state> |

Failed or retried attempts, with exits and log paths.

## Frontmatter probe diffs

| Case | Documented | Class | Control | Candidate | Disposition |
| --- | --- | --- | --- | --- | --- |

## Long-flow-line evidence

| Shape | Lane | Slope (largest four) | Largest-size min ns | Refusal rows | Demonstrated |
| --- | --- | --- | --- | --- | --- |

Limits (wall, CPU, repetitions). Timing is descriptive and not a gate.

## Sizes

| Artifact | Host Δ final | Host Δ gzip-9 | Projected CI gzip-9 | CI candidate gzip-9 | Ceiling | Headroom |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |

Tarball (host bytes, CI bytes or why they were not measured), glue, dist totals, the CI run
URL and SHA, and whether the owner exception applied (which artifacts exceeded 1,024 B).

## Decision and follow-up

The decision with its reasons; the #4161 state (open on HOLD, completes on KEEP); the #4173
state (not planned on KEEP; pin + deterministic coverage on MIGRATE-ELIGIBLE).
```
