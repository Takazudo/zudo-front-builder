---
description: "Release @takazudo/zfb by following the canonical RELEASING.md: judge the version, measure version-stamped sizes in CI, bump the eleven lockstep packages, write the five package changelog pages, push, verify required CI on the bump SHA, create the draft GH Release, admit the owner-Mac Darwin inputs (built with scripts/build-darwin-local.sh; x86_64 Rosetta-translated), publish without GitHub Latest, watch release.yml and the installer smoke, verify delivery, then promote Latest and update Homebrew. npm auth is OIDC trusted publishing only. STABLE-BY-DEFAULT: with no argument a patch/minor lands on npm `latest`; a major stops and asks. Stops (never works around) on missing Darwin inputs or an unacknowledged provenance downgrade. Pass --confirm to stop at the unpublished draft. Triggers on rough requests like \"bump version\", \"cut a release\", \"release zfb\", \"make a release\"."
user-invocable: true
argument-description: "Optional — no argument: judge the level from the commits and land a patch/minor stable (a major stops and asks). next — force a prerelease (-next.1 at the judged level from a stable, or -next.N+1 continuing a prerelease). major|minor|patch — force a prerelease on that component. stable — promote the current prerelease. stable major|minor|patch — force a specific stable bump. --confirm — present the proposal and wait, and stop at the unpublished draft. cancel — delete an orphaned draft GH Release instead of releasing. resume — re-run scripts/release/release-status.mjs and continue from its recommendation."
---

# /l-make-release

**Follow [`RELEASING.md`](../../../RELEASING.md) exactly.** It is the canonical, tool-neutral
procedure; this skill only maps arguments and adds Claude Code notes. Read the whole file before
the first mutation. Where this skill and `RELEASING.md` disagree, `RELEASING.md` wins.

## Arguments

- Version arguments map to `RELEASING.md` "A2. Version judgment".
- No `--confirm`: run autonomously; the invocation authorizes this one release, including the
  `make_latest` promotion after a delivery-verifier exit 0. It does **not** authorize acknowledging
  a provenance downgrade or a recovery dispatch with `allow_provenance_downgrade=true`; those are
  owner-only.
- `--confirm`: show the A3 proposal and wait before the first mutation; ask before deleting drafts;
  stop after A10 with the draft unpublished and print the remaining steps (Phase M, A11–A15) for
  the owner.
- `cancel`: go to "Cancel and clean up".
- `resume` (or any re-invocation mid-release): start at "Step 0: where am I?".

## Claude Code notes

- Start with `node scripts/release/release-status.mjs --tag v<version> --probe-capabilities` once the
  version is known, and run Step 0 again after every interruption or context compaction.
- Long waits (CI on the bump SHA, `release.yml`, the installer smoke) run in the background
  (`run_in_background: true`, or `/watch-ci` for the bump commit); never end the turn waiting.
- Heavy local commands go through `$HOME/.claude/scripts/heavy-guard.sh` when present (see
  `RELEASING.md` "Heavy local commands").
- **Phase M is Mac-only.** If this session runs on the owner's Mac, you may run
  `scripts/build-darwin-local.sh` as written there. On any other host, print the paste-ready Mac
  prompt from `RELEASING.md` and stop. Never fabricate or copy Darwin receipts.
- On a provenance-downgrade preflight result, stop and report; never edit the Release body to add
  `<!-- zfb-release: allow-provenance-downgrade -->`, and never create or edit
  `release/provenance-policy.json` — both are owner-only. Every local-Darwin release is a
  downgrade unless the owner's committed policy covers it.
- Publishing is `pack` (unprivileged: install, build, `npm pack`/`pnpm pack`) then `publish`
  (`id-token` only: `npm publish <tgz>` with explicit provenance; no install, no pnpm). Find
  `release.yml` runs by their exact name `Release v<version>[ (finalize only)][ (dry run)]`; the
  delivery verifier requires the `Pack npm tarballs` and `Upload MD/WASM digest asset` jobs.
- **A12 has two routes with the same gate (`publish-draft` + clear/owner-acknowledged A11).** On
  the owner's Mac with tag rights, use the local PATCH. In a Cloud session that cannot push tags
  (or whose `gh` is unauthenticated), use the Actions route: dispatch `release-coordinator.yml`
  from `main` with the frozen `version` + `sha`, first with `dry_run=true`. Use the GitHub MCP
  server's workflow dispatch if `gh` is unavailable, and read the run back. Never push a tag from
  another checkout, add a PAT or bypass protection to get around a refusal. A `TAG_CREATE_FAILED`
  is reported with its recorded diagnostics, not explained. Re-sending identical inputs resumes.
  The coordinator never re-runs a failed release; follow Step 0. Moving tag creation to Actions is
  not the whole release in the Cloud: Phase M, the downgrade decision and Homebrew stay with the
  owner and the Mac.
- After a coordinated `release.yml` run succeeds, check for its `workflow_run` installer smoke. If
  none appears, dispatch `node-free-smoke.yml` at the tag once (RELEASING.md A13).
- Keep JSON evidence outside the checkout (the session scratchpad or the repo-scoped cclogs dir)
  and name the paths in the final report.
- The report uses verdict headings (see `RELEASING.md` "Release report").
