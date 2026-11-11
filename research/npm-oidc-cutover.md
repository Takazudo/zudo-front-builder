# npm OIDC cutover — owner runbook (#4687)

Owner-only steps that complete the move to OIDC-only npm publishing. The repository side landed in
#4688 (no `NPM_TOKEN` anywhere; `scripts/npm-oidc-preflight.mjs` fails closed without OIDC or with
any stored npm credential; npm 12.2.1 on Node 24.22.1 in the publish job) and #4690 (the
procedure in [`RELEASING.md`](../RELEASING.md)). Nothing in this file is done by an agent on the
owner's behalf. Order: **(a) → (b) during the next authorized release → (c) before or at that
release → (d) only after (b) is proven.**

## (a) Trusted publisher on every package

Grants are **per package**: a package without its own trusted publisher cannot publish or move a
dist-tag, even when the other ten work. For each of the eleven packages, open the settings URL,
add a trusted publisher, and save:

| Field | Value |
| --- | --- |
| Publisher | GitHub Actions |
| Organization or user | `Takazudo` |
| Repository | `zudo-front-builder` |
| Workflow filename | `release.yml` |
| Environment | *(blank)* |
| Allow npm dist-tag | **enabled** — off by default; `scripts/advance-latest-dist-tag.sh` and `scripts/retire-next-dist-tag.mjs` fail with an auth error without it |

| Package | Settings |
| --- | --- |
| `@takazudo/zfb` | <https://www.npmjs.com/package/@takazudo/zfb/access> |
| `@takazudo/zfb-slugify` | <https://www.npmjs.com/package/@takazudo/zfb-slugify/access> |
| `@takazudo/zfb-runtime` | <https://www.npmjs.com/package/@takazudo/zfb-runtime/access> |
| `@takazudo/zfb-adapter-cloudflare` | <https://www.npmjs.com/package/@takazudo/zfb-adapter-cloudflare/access> |
| `@takazudo/zfb-md-wasm` | <https://www.npmjs.com/package/@takazudo/zfb-md-wasm/access> |
| `create-zfb` | <https://www.npmjs.com/package/create-zfb/access> |
| `@takazudo/zfb-darwin-arm64` | <https://www.npmjs.com/package/@takazudo/zfb-darwin-arm64/access> |
| `@takazudo/zfb-darwin-x64` | <https://www.npmjs.com/package/@takazudo/zfb-darwin-x64/access> |
| `@takazudo/zfb-linux-arm64-gnu` | <https://www.npmjs.com/package/@takazudo/zfb-linux-arm64-gnu/access> |
| `@takazudo/zfb-linux-x64-gnu` | <https://www.npmjs.com/package/@takazudo/zfb-linux-x64-gnu/access> |
| `@takazudo/zfb-win32-x64-msvc` | <https://www.npmjs.com/package/@takazudo/zfb-win32-x64-msvc/access> |

Constraints worth knowing:

- OIDC dist-tag writes need npm **12.2.0 or later**; the publish job pins 12.2.1.
- Trusted publishing works only on GitHub-hosted runners; self-hosted runners are unsupported.
- The identity is exact: renaming the workflow file, moving the publish into another workflow, or
  adding a GitHub environment breaks it until each package's publisher is updated.
- OIDC publishing from GitHub Actions attests automatically. That is why the Darwin carriers pass
  `--provenance=false` explicitly (pnpm 12.8.2 ignores `NPM_CONFIG_PROVENANCE=false` alone).

## (b) Live proof at the next authorized release

The cutover is unproven until a real release publishes through it. Dry runs, skipped steps,
`finalize_only` runs that publish nothing, and public registry reads do **not** prove OIDC
publication or dist-tag writes. Record for that release:

- [ ] `release.yml` run ID, attempt and the `publish` job ID; the `npm-oidc-preflight` step passed
      on attempt 1 (or every retry recorded).
- [ ] Each of the eleven `npm publish` invocations used OIDC (the job log shows no token auth and
      no stored credential; the preflight would have failed otherwise).
- [ ] `scripts/verify-npm-provenance-expectations.mjs` passed on the publish job's expectations
      file: nine packages `attested`, both Darwin carriers `none`, exactly as intended.
- [ ] Independent read: `node scripts/release/npm-package-state.mjs --version <v>
      --source-commit <sha> --admission <admission.json> --json` reports `present-verified` for
      all eleven.
- [ ] Dist-tag writes happened through OIDC: for a stable, `retire-next-dist-tag.mjs` output in the
      log (a removal, or "still ahead"/"absent" when nothing needed changing — record which; a
      no-op does not prove the write permission). For a release where `ALSO_LATEST` applies,
      `advance-latest-dist-tag.sh` output. If no dist-tag write was needed, the dist-tag grant
      remains **unproven** until a release that needs one.
- [ ] `npm view <pkg> dist-tags --json` for all eleven matches the CLAUDE.md dist-tag rule.
- [ ] `scripts/verify-release-delivery.mjs` exit and JSON path.

Unknown evidence stays unknown. A failure is recorded with its first result; do not re-run until
something passes and call it proven.

## (c) Darwin provenance downgrade decision

Through 6.0.1, both `@takazudo/zfb-darwin-arm64` and `@takazudo/zfb-darwin-x64` were built on
hosted macOS runners and published **with** provenance. From the first release built under
#4687 they are built on the owner's Mac and published **without** provenance, because an Actions
attestation would falsely claim an Actions build of bytes the Mac produced.

Consequence: npm records provenance per version, permanently. A consumer with pnpm
`trustPolicy: no-downgrade` (pnpm ≥ 10.21; pnpm 11.2+ also re-checks an existing lockfile under
`--frozen-lockfile`) fails to install that version with `ERR_PNPM_TRUST_DOWNGRADE`. Because pnpm
resolves all `optionalDependencies`, this affects Linux and Windows consumers of `@takazudo/zfb`
too, not only Mac users. It continues for every later version until a Darwin package is attested
again; the downgrade is judged against the earlier attested versions.

`release.yml`'s preflight is default-deny: the release blocks before publishing until the owner
acknowledges by adding this exact line to the draft Release body (or, for a dispatch, setting
`allow_provenance_downgrade=true`):

```text
<!-- zfb-release: allow-provenance-downgrade -->
```

The decision is the owner's alone. Options: acknowledge and publish (and add release-note and
troubleshooting guidance for affected consumers), or hold the release. The Darwin receipts and the
CI admission bind the published bytes to the source and the Mac qualification, but they are not an
npm attestation, and pnpm's trust policy does not read them.

## (d) Retire the old credentials — only after (b) is proven

There is no renamed credential and no fallback: after this step a token publish must be
impossible.

1. **Find other users of a shared `NPM_TOKEN`** before deleting anything:

   ```sh
   gh secret list --repo Takazudo/zudo-front-builder
   gh repo list Takazudo --limit 500 --json nameWithOwner --jq '.[].nameWithOwner' |
     while read -r repo; do
       gh secret list --repo "$repo" 2>/dev/null | grep -q '^NPM_TOKEN' && echo "$repo"
     done
   gh secret list --org zudolab 2>/dev/null | grep '^NPM_TOKEN'
   gh api repos/Takazudo/zudo-front-builder/environments --jq '.environments[].name'
   ```

   Repeat the per-repo loop for every other owner or organization that publishes packages; check
   environment secrets with `gh secret list --repo <repo> --env <env>`. A repository still using
   the same token keeps it alive — migrate it or rotate that repository onto its own credential
   before revoking.
2. **Delete the GitHub secret** in this repository:
   `gh secret delete NPM_TOKEN --repo Takazudo/zudo-front-builder`.
3. **Revoke obsolete npm write tokens:** `npm token list` from an interactive login, then
   `npm token revoke <id>` for each publish-capable token no longer needed (or on
   <https://www.npmjs.com/settings/~/tokens>). This includes tokens stored in `~/.npmrc` on the
   owner's machines — **this Mac's `~/.npmrc` holds one**. Check without printing it
   (`grep -c '_authToken' ~/.npmrc`), revoke it on npmjs.com, then remove the line.
4. **Disallow tokens per package:** on each package's settings page above, set Publishing access
   to **"Require two-factor authentication and disallow tokens"**. Trusted publishing keeps working;
   token publishes stop. Re-read each page afterwards.
5. Record what was deleted, revoked and set (names and dates only, never token values).

## (e) Codex Cloud acceptance prompt (owner-started)

Run this only as part of a **real release the owner has authorized**. It is the acceptance test of
the any-environment phase; a Mac-only run never counts as Cloud acceptance. Paste into a fresh
Codex Cloud task on this repository, filling in the version:

```text
You are performing an owner-authorized release of zfb v<version> as the Cloud acceptance of
RELEASING.md (#4687). Read .agents/skills/zfb-release/SKILL.md, CLAUDE.md's release sections and
all of RELEASING.md first, then follow RELEASING.md exactly.

1. Capability: run `node scripts/release/release-status.mjs --tag v<version> --probe-capabilities`
   and report gh auth, repo permission and workflow access. If push to main, Release write or
   workflow dispatch is missing, stop and report; that is an acceptance result, not something to
   work around.
2. Exact source: prepare the release from a fresh checkout per A1-A10 (version judgment, CI-measured
   version stamps, bump, five changelog pages, focused checks, push, verify-source-ci.mjs on the
   bump SHA, draft Release targeting it). Record source SHA/tree and your tested checkout/tree.
3. Darwin inputs: run Step 0 and the A11 admission on the draft's assets (before publication
   Step 0 shows blocked:tag-missing; that alone is expected). If it recommends prepare-darwin, or
   admission rejects or misses any of
   the ten Darwin inputs, print RELEASING.md's paste-ready Mac prompt and STOP. Missing local
   native receipts are a blocker. Never build, emulate or write Darwin evidence, and never use a
   hosted macOS runner.
4. Provenance: run the mac-local preflight. On a downgrade, STOP and report; never add the
   acknowledgement marker or dispatch with allow_provenance_downgrade=true.
5. Publication: only when RELEASING.md A12's pre-publish gates hold, publish per A12 (stable without Latest
   in one update), watch release.yml and the installer smoke, and record run/attempt/job IDs with
   first results and retries.
6. Completion: run verify-release-delivery.mjs, npm-package-state.mjs and the provenance
   expectations evidence; promote Latest only on exit 0; report Homebrew as an owner command if
   you lack tap access. If a post-publish step failed, follow Step 0 (dispatch-finalize via an
   explicit `gh workflow run ... -f dry_run=false -f finalize_only=true`, or report
   dispatch-recovery for the owner).
7. Report with verdict headings: what was proven in Cloud, what was blocked and why, and the
   evidence checklist from research/npm-oidc-cutover.md section (b). Pending, skipped, dry-run and
   pass-on-retry results are not passes.
```
