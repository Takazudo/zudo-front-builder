# npm OIDC cutover — owner runbook (#4687)

Owner-only steps that complete the move to OIDC-only npm publishing. The repository side landed in
#4688 (no `NPM_TOKEN` anywhere; `scripts/npm-oidc-preflight.mjs` fails closed without OIDC or with
any stored npm credential; npm 12.2.1 on Node 24.22.1 in the publish job) and #4690 (the
procedure in [`RELEASING.md`](../RELEASING.md)). Nothing in this file is done by an agent on the
owner's behalf. Confirm (a) and (c) before or at the next authorized release, collect the live
evidence in (b) during that release, and proceed to (d) only after (b) is proven.

## (a) Trusted publisher on every package

Grants are **per package**: a package without its own trusted publisher cannot publish or move a
dist-tag even when every other package can. For each active package in the selected release
SOURCE, open its settings URL, add a trusted publisher, and save. The table below lists the
eleven active packages on current main.

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

Use this checklist at a separately authorized legitimate release. Count only fresh publishes and
actual dist-tag mutations as evidence of those OIDC writes. Dry runs, existing-version skips,
`finalize_only` runs that publish nothing, and public registry reads supply no publication proof.
A finalizer can supply dist-tag write evidence when it actually changes a tag.

The [v7.1.0 ledger](https://github.com/Takazudo/zudo-front-builder/issues/4691#issuecomment-7067197306)
records real OIDC publishes and finalizer dist-tag writes, the original publish job's first
provenance verification failure (exit 3), and the delivery verifier's exit 1. Account
configuration, credential retirement and Cloud initiation remained unobserved. Overall cutover
acceptance remains open in [#4691](https://github.com/Takazudo/zudo-front-builder/issues/4691).

Use the immutable release SOURCE commit to resolve the active package inventory and publication
policy version through [the source policy resolver](../scripts/release/source-publication-policy.mjs).
Derive counts from that inventory and provenance expectations from the publication mode and
observed state of any already-published versions. In a normal `mac-local` run, new CI-built
packages expect provenance and new owner-Mac Darwin carriers expect none. For new packages,
`all-provenance` expects attestation across the set and `recovery-no-provenance` expects none. For
packages already present, use the observed provenance expectation recorded by the publisher.

With source policy v2, every active package's packed `package.json.gitHead` must equal the exact
verified SOURCE commit. The [packer](../scripts/release/pack-release-tarballs.mjs) verifies its
checkout against SOURCE, stamps each input manifest before packing and computing tarball integrity,
and restores the original manifest bytes even if packing fails. The [prepublication validator](../scripts/release/verify-release-tarballs.mjs)
independently checks SHA-512 and reads the actual tarball contents, including the packed `gitHead`.
Historical sources with v1 or no policy file keep their original packing and admission contracts.
Use the evidence recorded for those releases under their original requirements.

- [ ] Record the `release.yml` run ID and attempt (run name exactly `Release v<version>`), and the
      job IDs of `Pack npm tarballs` and `publish`. Record the first `npm-oidc-preflight` result
      and every retry.
- [ ] Record the `publish` job's tarball integrity and content validation results, its pinned npm
      version (currently **12.2.1**), the actual `npm publish <tgz>` operations through OIDC and
      their explicit provenance flags, and any existing-version skips.
- [ ] For every active package subject to the v2 contract, record the observed registry
      `gitHead` and compare it with the exact SOURCE SHA. Missing, malformed or foreign values
      are unresolved findings even when another identity check succeeds. Packing and independent
      prepublication validation prove the tarball metadata offline. The live registry acceptance
      remains tracked in [#4803](https://github.com/Takazudo/zudo-front-builder/issues/4803#issuecomment-7135475528).
- [ ] Record the first result of `scripts/verify-npm-provenance-expectations.mjs` on the publish
      job's expectations file, with its per-package `attested` or `none` expectations and every
      retry or separate finalizer result.
- [ ] Independent read: `node scripts/release/npm-package-state.mjs --version <v> --source-commit
      <sha> --admission <admission.json> --json` reports `present-verified` for every active
      package. Record the reported identity method and evidence. The tool checks a supported
      SLSA attestation's SOURCE commit and subject digest first. It adds integrity to that
      method when supplied packed integrity equals registry `dist.integrity`. Without a
      supported attestation it tries the exact integrity match, then registry `gitHead`, then
      admitted Darwin payload digests when applicable. A rejected supported attestation or a
      foreign `gitHead` checked as the identity method stops verification. The tool checks
      the downloaded tarball's SHA-512 against registry `dist.integrity` and verifies admitted
      Darwin binary and addon digests when the applicable admission verdict is supplied, even
      when another identity method succeeds. `gitHead` supplies
      source metadata only. Provenance, artifact integrity and consumer admission have their
      own requirements.
- [ ] Record OIDC dist-tag operations and the job that performed them. Include channel
      promotion after provenance verification (`latest` for stable, `next` for prerelease,
      plus `latest` when `ALSO_LATEST` applies), and applicable sweeps of the holding tag and
      stale `next`. Record actual mutations separately from no-ops such as an absent or
      still-ahead tag. If no dist-tag write was needed, the dist-tag grant remains
      **unproven** until a release that needs one.
- [ ] `npm view <pkg> dist-tags --json` for every active package matches the CLAUDE.md dist-tag rule.
- [ ] `scripts/verify-release-delivery.mjs` exit and JSON path.

Unknown evidence stays unknown. A failure is recorded with its first result; do not re-run until
something passes and call it proven.

## (c) Darwin provenance downgrade decision

Through 6.0.1, both Darwin carriers were built on hosted macOS runners and published **with**
provenance (`@takazudo/zfb-darwin-arm64` attested since 1.0.0, `@takazudo/zfb-darwin-x64` since
2.13.0). From #4687 on they are built on the owner's Mac and published **without** provenance,
because an Actions attestation would falsely claim an Actions build of bytes the Mac produced.

Earlier versions keep their attestations, so each owner-Mac release has the same potential
trust downgrade for consumers. In the [recorded ZFB 7.2.0 installation](https://github.com/Takazudo/zudo-front-builder/issues/4691#issuecomment-7130227216),
pnpm 12.4.1 with `trustPolicy: no-downgrade` and `trustPolicyIgnoreAfter: 10080` exited
successfully while reporting trust-related optional dependency resolution failures and omitting
both Darwin carriers from the lockfile. On arm64 macOS the native CLI then failed because its
carrier was absent. An absent carrier alone leaves both the cause and the package's publication
status unverified.

Darwin receipts and CI admission bind the payload to SOURCE and the owner's Mac qualification.
That admission and the owner's acknowledgement permit publication without npm provenance.
The consumer's package manager applies its own trust policy. Trust-age settings and deliberate,
scoped exclusions can affect admission while the published provenance stays the same. A lockfile
that omitted carriers can require explicit re-resolution of the already selected ZFB version.
See the [English troubleshooting guide](../docs/src/content/docs/guides/troubleshooting.mdx#missing-darwin-binary-with-pnpm-trust-policy)
and the [Japanese troubleshooting guide](../docs/src/content/docs-ja/guides/troubleshooting.mdx#pnpm-の-trust-policy-で-darwin-バイナリが見つからない場合)
for diagnosis and configuration details.

The release preflight is default-deny for an unacknowledged downgrade. The owner can acknowledge
one per run with the exact Release body marker `<!-- zfb-release: allow-provenance-downgrade -->`
on its own line or the `allow_provenance_downgrade=true` dispatch input, or use a durable
decision under the [provenance contract](../RELEASING.md#provenance-contract-and-the-downgrade-blocker).
Main-based recovery requires the owner-only dispatch input for an acknowledged downgrade.

The owner has already committed [the durable policy](../release/provenance-policy.json) linked
to [#4687](https://github.com/Takazudo/zudo-front-builder/issues/4687). It covers both locally
built Darwin carriers in `mac-local` mode from 6.1.1 onward. The release process reads it from
committed `main` via `origin/main`. The package inventory and v2 metadata requirement come from
the immutable release SOURCE. For a covered downgrade, record the preflight's `ACKNOWLEDGED`
lines. A downgrade outside those decisions remains subject to default-deny and the owner-only
acknowledgement rules.

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
3. **Darwin provenance policy (M1).** The owner's committed decision already covers the normal
   owner-Mac releases described in section (c).
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
