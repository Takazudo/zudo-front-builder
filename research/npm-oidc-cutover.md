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
  `--provenance=false` explicitly. Publishing is `npm publish <tgz>` from the `publish` job only;
  pnpm packs workspace packages in the unprivileged `pack` job but never publishes.

## (b) Live proof at the next authorized release

The cutover is unproven until a real release publishes through it. Dry runs, skipped steps,
`finalize_only` runs that publish nothing, and public registry reads do **not** prove OIDC
publication or dist-tag writes. Record for that release:

- [ ] `release.yml` run ID and attempt (run name exactly `Release v<version>`), and the job IDs of
      `Pack npm tarballs` and `publish`; the `npm-oidc-preflight` step passed on attempt 1 (or
      every retry recorded).
- [ ] The `publish` job verified each tarball's sha512 against the pack manifest, then ran
      `npm publish <tgz>` eleven times under npm **12.2.1** through OIDC (no token auth, no stored
      credential; the preflight would have failed otherwise), with an explicit `--provenance` on the
      nine CI-built packages and `--provenance=false` on the two Darwin carriers. This is the
      first live evidence that publishing a **tarball** (not a directory) with OIDC provenance
      works on that npm version; record it either way.
- [ ] Tarball publishes record no `gitHead` in the registry metadata. Identity therefore comes
      from the attestation's source `gitCommit` (nine attested packages) and, for the Darwin
      carriers, from the binary/addon digests matching the admission verdict — record that
      `npm-package-state.mjs` used those routes, not `gitHead`.
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

Through 6.0.1, both Darwin carriers were built on hosted macOS runners and published **with**
provenance (`@takazudo/zfb-darwin-arm64` attested since 1.0.0, `@takazudo/zfb-darwin-x64` since
2.13.0). From #4687 on they are built on the owner's Mac and published **without** provenance,
because an Actions attestation would falsely claim an Actions build of bytes the Mac produced.

Consequence: npm records provenance per version, permanently. A consumer with pnpm
`trustPolicy: no-downgrade` (pnpm ≥ 10.21; pnpm 11.2+ also re-checks an existing lockfile under
`--frozen-lockfile`) fails to install that version with `ERR_PNPM_TRUST_DOWNGRADE`. Because pnpm
resolves all `optionalDependencies`, this affects Linux and Windows consumers of `@takazudo/zfb`
too, not only Mac users. The earlier attested versions stay attested forever, so **every**
local-Darwin release is a downgrade, not only the first: those consumers can never install a new
Darwin carrier version while this build model holds.

`release.yml`'s preflight is default-deny: each such release blocks before publishing until the
owner acknowledges it, by one of two routes (see
[RELEASING.md](../RELEASING.md#provenance-contract-and-the-downgrade-blocker) for the schema and
limits):

- **Per run:** this exact line in the draft Release body, or `allow_provenance_downgrade=true` on a
  dispatch:

  ```text
  <!-- zfb-release: allow-provenance-downgrade -->
  ```

- **Durable:** an owner-authored, committed `release/provenance-policy.json`
  (`zfb-provenance-policy/v1`), one decision per Darwin package with `unattested_from_version`,
  `reason`, `decided_by` and `issue`. It applies only in `mac-local` mode, only to the run's
  locally built Darwin carriers, only from that version upward, and is read only from `main`'s
  committed tree (`origin/main`), never from a working tree or the release commit. It is absent
  by default; agents never create or edit it.

The decision is the owner's alone (option 3 under "Owner options and follow-ups"). Options:
acknowledge per release or once by policy (and add release-note and troubleshooting guidance for affected consumers), or hold the
release. The Darwin receipts and the
CI admission bind the published bytes to the source and the Mac qualification, but they are not an
npm attestation, and pnpm's trust policy does not read them.

## Owner options and follow-ups

The owner decides each of these; agents never change npm, GitHub or repository settings for them.

1. **Protected environment for the trusted publisher (follow-up, not done).** Each package's
   trusted publisher could be bound to a GitHub environment that is protected and restricted to
   `v*` tags and `main`, so only those refs can obtain a publish token. This needs the workflow to
   declare that `environment:` on the `publish` and `finalize` jobs at the same time as the npm
   setting changes; changing either alone breaks publishing. It is a coordinated owner + code
   change and is tracked as a follow-up.
2. **Immutable Releases (currently off).** A read-only check shows the repository setting
   `enabled: false`. Turning it on conflicts with the current flow, in which `release-assets` (and
   `md-wasm-asset`) upload assets after the Release is published; the flow would have to be
   reordered so every asset is attached before publication first.
3. **Darwin provenance policy (M1).** Section (c): acknowledge per release, commit
   `release/provenance-policy.json`, or hold.
4. **Receipts are unsigned.** Darwin qualification receipts and their admission bind bytes to the
   source, not to an operator: anyone with Release write access could upload a self-consistent
   receipt. Release write access is the trust anchor; keep it limited to the owner.

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
3. Darwin inputs: run Step 0 (source kind draft-target before publication) and the A11
   admission cross-check on the draft's assets. If Step 0 recommends prepare-darwin, or
   admission rejects or misses any of
   the ten Darwin inputs, print RELEASING.md's paste-ready Mac prompt and STOP. Missing local
   native receipts are a blocker. Never build, emulate or write Darwin evidence, and never use a
   hosted macOS runner.
4. Provenance: run the mac-local preflight. On an unacknowledged downgrade, STOP and report; never
   add the acknowledgement marker, dispatch with allow_provenance_downgrade=true, or create or
   edit release/provenance-policy.json.
5. Publication: only when Step 0 recommends publish-draft, publish per A12 (stable without Latest
   in one update), watch the run named exactly `Release v<version>` (jobs including
   `Pack npm tarballs`, `publish` and `Upload MD/WASM digest asset`) and the installer smoke, and
   record run/attempt/job IDs with first results and retries.
6. Completion: run verify-release-delivery.mjs, npm-package-state.mjs and the provenance
   expectations evidence; promote Latest only on exit 0; report Homebrew as an owner command if
   you lack tap access. If a post-publish step failed, follow Step 0 (dispatch-finalize via an
   explicit `gh workflow run ... -f dry_run=false -f finalize_only=true`, or report
   dispatch-recovery for the owner).
7. Report with verdict headings: what was proven in Cloud, what was blocked and why, and the
   evidence checklist from research/npm-oidc-cutover.md section (b). Pending, skipped, dry-run and
   pass-on-retry results are not passes.
```
