---
name: zfb-release
description: "Release @takazudo/zfb and its lockstep packages by following the repository's canonical RELEASING.md (version judgment, CI-measured version stamps, bump, five-lane changelog, required CI on the bump SHA, draft Release, Darwin admission, publish, watch, delivery verification, Latest promotion, Homebrew). Use only when the owner has authorized a specific release. Cloud sessions consume owner-Mac Darwin inputs and stop when they are missing."
---

# zfb release (Codex)

Follow [`RELEASING.md`](../../../RELEASING.md) at the repository root **exactly**. This file only
adds Codex-specific notes; where they disagree, `RELEASING.md` wins.

## Before anything else

1. Codex does not load `CLAUDE.md` automatically. Read its "npm dist-tags", "Five-lane release
   changelog contract" and "Windows on CI runners, macOS on the owner's Mac" sections, then all of
   `RELEASING.md`.
2. Confirm the owner authorized **this** release (version or level) in the task. A green CI run,
   a merged PR or a development instruction is not release authorization.
3. Prove GitHub capability before mutating anything:

   ```sh
   node scripts/release/release-status.mjs --tag v<version> --probe-capabilities
   ```

   Push to `main`, Release write and Actions dispatch must all be available for the steps you will
   run. Missing capability is a stop: report it; never change credentials, secrets or protection.
   A refused tag push is **not** missing capability for A12. Use the Actions route
   (`release-coordinator.yml`, RELEASING.md "A12 (Actions route)") through whatever can dispatch
   a workflow and read runs, such as `gh` or a GitHub MCP server. When nothing can, print the
   exact dispatch for the owner. An unauthenticated `gh` is never a reason to push a tag from
   another checkout.

## Cloud rules

- **Never create Mac evidence.** Phase M runs only on the owner's Mac. When
  `release-status.mjs` recommends `prepare-darwin`, or Darwin admission (A11) rejects or misses any of
  the ten inputs, print the paste-ready Mac prompt from `RELEASING.md` and stop. Do not build
  Darwin binaries, emulate them, write receipts, or add a hosted macOS runner.
- **Stop on the provenance-downgrade blocker.** Never add
  `<!-- zfb-release: allow-provenance-downgrade -->` to a Release body and never dispatch with
  `allow_provenance_downgrade=true`; never create or edit `release/provenance-policy.json`
  (the owner's durable acknowledgement). Every local-Darwin release is a downgrade unless that
  committed policy covers it. The owner's policy on `main` covers both Darwin carriers from 6.1.1,
  so a preflight that exits 0 with two `ACKNOWLEDGED` lines is clear: record them and continue.
  Report the preflight output for the owner only when a downgrade is not covered.
- Publication runs in two jobs: `pack` (no `id-token`; installs, builds, `npm pack` for platform
  packages and `pnpm pack` for workspace packages) and `publish` (`id-token` only; `npm publish
  <tgz>` with explicit `--provenance`/`--provenance=false` under npm 12.2.1; pnpm never publishes).
  Runs are named `Release v<version>[ (finalize only)][ (dry run)]`; the delivery verifier expects
  the `Pack npm tarballs` and `Upload MD/WASM digest asset` jobs.
- `wait-npm-processing` from Step 0 (or a verifier exit 3) means npm is still processing a package:
  stop and report, never recover. See `RELEASING.md` "npm is still processing a package".
- Run Step 0 (`release-status.mjs`) at the start and after every interruption; act on its single
  recommendation.
- Follow-up workflow runs are explicit `gh workflow run release.yml --ref main ...` dispatches
  with `dry_run` always passed explicitly (its default is false). The coordinator's own dispatch
  of `release.yml` at the tag is the one exception, and only the coordinator makes it.
- The coordinator (`dry_run` defaults to true) needs the frozen `version` and the full source
  `sha`.
  - It never substitutes a newer `main`, and a resume keeps the same SHA.
  - It stops before any write on missing Mac inputs or an unacknowledged downgrade. That is a stop
    for you too.
  - Re-sending identical inputs resumes. A `TAG_CREATE_FAILED` is reported with its recorded
    diagnostics; never add a PAT.
  - A Cloud-started publication is "tag creation in Actions", not "the whole release in the
    Cloud". Phase M, the downgrade decision, Homebrew, and the A8 push without admin bypass stay
    with the owner.
- Heavy commands use `HEAVY_GUARD` or a guard under `$HOME/.codex/scripts/` when present, else run
  directly. Check free disk before any full Rust build (an observed Cloud filesystem was 32 GB).
- Homebrew needs push access to `Takazudo/homebrew-tap`; without it, report the exact
  `./scripts/update-homebrew-formula.sh v<version> --push` command for the owner.
- Record source SHA/tree, tested checkout, control SHA for dispatches, and every run/attempt/job ID
  with first results and retries. Pending, cancelled, skipped, dry-run and pass-on-retry are not
  passes.
