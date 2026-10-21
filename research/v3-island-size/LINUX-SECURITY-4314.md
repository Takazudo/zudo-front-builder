# Linux x64 stage for the #4314 security transition

This is the Linux x64 counterpart of [LOCAL-SECURITY-4314.md](LOCAL-SECURITY-4314.md). It **stages** the same reviewed `rustls-security-4314` Cargo producer-lock transition for `decision-linux-x64.json` and stops at independent review. Nothing here applies a stage, writes a live contract, merges or releases. Running it, and later applying it, each need their own authorization.

`cargo-producer-lock` is not a paired reason, so the Linux stage lives in its own stage directory and does not depend on the Darwin stage. Both platforms still pin the old lock, so neither contract admits the new lock until both are applied.

## Evidence already collected

No new Actions run or native build is needed. The ordinary `health` job already retained complete Linux evidence at the new lock:

| Field | Value |
| --- | --- |
| Run / job | [health run 39302891207](https://github.com/Takazudo/zudo-front-builder/actions/runs/39302891207), job `118232731946` (`ubuntu-24.04`) |
| Artifact | `island-size-linux-x64`, id `12213032790`, `sha256:369e9abfd79bbf08e610e4bf68eb5d99eb87d4f5cd7eb4994f0ec562b1e5f00b`, expires `2026-10-26T18:46:58Z` |
| Measured source | PR #4326 merge source `e397161c609e780a79ec4273c907168355518aab` |
| Probe head | `0355ad78a4f8d2c240844d2b48da7c2769f6de25` |
| Tree | `883733d5a01a0b4cd579c8e204c3e7d904dbff6a`, identical to the merge source, the probe head and `base/4314-security-darwin-evidence` at `7e79458304878d51bafd29e24ec25e93b84893ca` |
| Recorded identity | package `4.5.3`, Cargo.lock `563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd`, pnpm lock `370bf32469ed4f3093a24aed1cab3dd68a28c58051847d425fcbe3d062fc30ee`, Node `v24.14.0`, zlib `1.3.1-e00f703`, linux/x64 |

The job's `Collect Linux x64 island-size measurements` step passed. Its `Enforce Linux x64 island-size budgets` step rejected only the three old-lock diagnostics and retained complete unchanged-total evidence:

```text
Island size budget rejected:
- local Cargo.lock: SHA-256 differs from the reviewed contract
- workspace provenance: Cargo lockfile differs from the contract
- packed provenance: Cargo lockfile differs from the contract
Complete unchanged-total evidence retained; budget verdict remains 1.
```

The run's other red step, `pnpm test:workspace`, is the known lock-pin assertion in `island-size-budget.test.mjs` and stays red until the transition is applied. If the artifact expires before staging, collect a fresh one with `island-size-native` (`platform=linux-x64`, a standard hosted runner) and replace every identifier above. Never reuse these identifiers for another artifact.

## Host and resources

This runs on any linux/x64 host, including Claude Code Cloud. It needs no Cargo build, so the Cloud disk limit for full Rust builds does not apply.

- The official Node `v24.14.0` linux-x64 build, which reports zlib `1.3.1-e00f703`. The stager recomputes the normalized sidecars, and a different Node patch or zlib refuses as a stale sidecar. Use it process-locally, without replacing the host's default Node.
- pnpm `12.8.2`.
- About 1 GiB of disk (the Node tarball, `node_modules`, a 0.7 MB artifact and a small stage) and well under 1 GiB of RAM. It takes minutes.
- Network access to nodejs.org, the npm registry and the GitHub artifact download.

## Stage

Run in one fresh Bash shell. `RETAIN` is a new directory outside the checkout.

```bash
set -euo pipefail
REPO='/absolute/path/to/clean/zudo-front-builder'
RETAIN='/absolute/path/to/new-linux-4314-stage'
NODE_DIR='/absolute/path/to/node-v24.14.0-linux-x64'
TOOL_SHA=7e79458304878d51bafd29e24ec25e93b84893ca
SOURCE_SHA=e397161c609e780a79ec4273c907168355518aab
HEAD_SHA=0355ad78a4f8d2c240844d2b48da7c2769f6de25
ARTIFACT_SHA=369e9abfd79bbf08e610e4bf68eb5d99eb87d4f5cd7eb4994f0ec562b1e5f00b
export PATH="$NODE_DIR/bin:$PATH"

test ! -e "$RETAIN"; mkdir -p "$RETAIN"
test "$(uname -s):$(uname -m)" = Linux:x86_64
test "$(node --version)" = v24.14.0
test "$(node -p process.versions.zlib)" = 1.3.1-e00f703
test "$(pnpm --version)" = 12.8.2

cd "$REPO"
git fetch origin "$SOURCE_SHA" "$HEAD_SHA" 21d6807e442bbed61e578c165a91d64c4a7dc141
test "$(git rev-parse HEAD)" = "$TOOL_SHA"
test -z "$(git status --porcelain --untracked-files=normal)"
tree="$(git rev-parse HEAD^{tree})"
test "$(git rev-parse "$SOURCE_SHA^{tree}")" = "$tree"
test "$(git rev-parse "$HEAD_SHA^{tree}")" = "$tree"
test "$(sha256sum Cargo.lock | cut -d' ' -f1)" = 563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd

pnpm install --frozen-lockfile --ignore-scripts > "$RETAIN/install.log" 2>&1
test -z "$(git status --porcelain --untracked-files=normal)"

# Download artifact 12213032790 (any authorized GitHub client) to $RETAIN/artifact.zip.
test "$(sha256sum "$RETAIN/artifact.zip" | cut -d' ' -f1)" = "$ARTIFACT_SHA"
mkdir "$RETAIN/artifact"
unzip -q "$RETAIN/artifact.zip" -d "$RETAIN/artifact"

if node scripts/rebaseline-island-size-linux.mjs \
  --artifact "$RETAIN/artifact" \
  --run-id 39302891207 --job-id 118232731946 \
  --artifact-id 12213032790 --artifact-expires-at 2026-10-26T18:46:58Z \
  --head-sha "$HEAD_SHA" --source-sha "$SOURCE_SHA" \
  --reason cargo-producer-lock --stage "$RETAIN/stage" \
  --expected-old-cargo-sha 230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138 \
  --expected-new-cargo-sha 563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd \
  --old-cargo-ref 21d6807e442bbed61e578c165a91d64c4a7dc141:Cargo.lock \
  --added-packages rustls@0.23.46 \
  --reviewed-cargo-transition research/v3-island-size/cargo-transitions/rustls-security-4314.json \
  > "$RETAIN/stage.log" 2>&1; then
  stage_status=0
else
  stage_status=$?
fi
printf '%s\n' "$stage_status" > "$RETAIN/stage.exit-code"
cat "$RETAIN/stage.log"
test "$stage_status" = 0
test -f "$RETAIN/stage/stage-manifest.json"
test -z "$(git status --porcelain --untracked-files=normal)"
```

Stop on any failed check. A stager refusal is a finding: keep the log and exit code, and do not edit the artifact, contract or arguments to get past it.

## Expected stage

- Exit 0, and live files unchanged (`git status` clean).
- The staged `decision-linux-x64.json` moves `toolchain.cargoLockSha256` from `230f0acb…` to `56319425…` and records `cargoTransition` with review id `rustls-security-4314`, the measured source `e397161c…` with probe head `0355ad78…`, and run, job and artifact `39302891207` / `118232731946` / `12213032790`.
- Every ceiling is unchanged and the allowance stays raw 0 / gzipNormalized 0. Only actual-gzip evidence may move by a few bytes.
- The staged README section and the Linux pins in `scripts/__tests__/island-size-budget.test.mjs` describe the same transition.

## After the stage

Return the stage log and exit code, `stage-manifest.json` and the staged diff with their SHA-256 digests. An independent review compares the staged diff with this expectation. Applying the stage (`--apply`) and any merge to `main` are separate owner decisions. After applying, the generated test comment that attributes `cargoLockSha256` to the cargo-internal-rewire (#4258) is stale. Correct only that comment, by hand, in the same reviewed change; change no assertion.

## After the Darwin apply

This section describes the apply. It does not authorize one.

Both stages write `README.md` and `scripts/__tests__/island-size-budget.test.mjs`, and each records those files' pre-apply hashes. Whichever applies second refuses as "moved since staging". The stage in [stage-evidence/linux-4314](stage-evidence/linux-4314/RESULT.md) was taken at `7e79458`, so it cannot be applied after the Darwin apply. Never edit a stage manifest to get past this.

Once the Darwin apply is merged, re-stage Linux from the same retained artifact. That is possible only while it is downloadable, until `2026-10-26T18:46:58Z`.

- Use the block above with two changes:
  - `TOOL_SHA` is the merged head.
  - Drop the two tree-equality checks. Those checks are this recipe's own, not the stager's: the merged tree no longer equals the measured source.
- The artifact's recorded Cargo.lock must still be the checkout's lock (`56319425…`).
- Keep every stager argument unchanged.
- Then `--apply` the new stage from the merged head, in the same change as the hand-pinned test edits.

If the artifact expires first, collect a fresh one as described above.
