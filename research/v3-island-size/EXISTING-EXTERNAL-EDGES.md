# Existing external dependency edges: capability, not activation

This extension supports the initialization compatibility edge discussed in
[#4301](https://github.com/Takazudo/zudo-front-builder/issues/4301) and
[draft #4307](https://github.com/Takazudo/zudo-front-builder/pull/4307).
An existing unpublished internal package can gain an edge to an existing,
unchanged external package without adding a package or upgrading a version.
The default Deno producer and the direct-only CLI graph remain separate proofs.

The implementation changes no Cargo.lock, approved contract, ceiling, allowance,
or live transition declaration. No admission is staged or applied by this work.
Tests use synthetic locks and artifacts in disposable temporary checkouts; their
paired staging/application exercises are not native evidence or live activation.

Policy: `CLAUDE.md` development strategy. Integration parent: main. The manager
owns focused tests, independent review, draft PR and exact-head CI diagnosis.
Application is a separate, subsequently authorized operation after evidence review.
D's cancellation barrier and dev tests are outside this branch's ownership.

## Exact declaration and refusal boundary

Use the existing `cargo-internal-rewire` reason with declaration **schema 2**.
Contract schema remains 3. Schema 1 retains its internal-only rule and refuses
`externalEdges`; merely supplying the new field cannot opt an old declaration in.
Schema 2 requires every actual added edge to appear exactly once in
`externalEdges`, with its internal source, literal Cargo.lock dependency spelling,
resolved external target and exact target-record SHA-256:

```json
{
  "schemaVersion": 2,
  "producer": "zfb",
  "oldCargoLockSha256": "<old whole-lock SHA-256>",
  "newCargoLockSha256": "<new whole-lock SHA-256>",
  "added": [],
  "changed": [{
    "id": "zfb-js-host@0.0.0",
    "manifest": "crates/zfb-js-host/Cargo.toml",
    "oldSha256": "<exact old internal record SHA-256>",
    "newSha256": "<exact new internal record SHA-256>",
    "removedDependencies": [],
    "addedDependencies": ["deno_core"]
  }],
  "externalEdges": [{
    "from": "zfb-js-host@0.0.0",
    "dependency": "deno_core",
    "to": "deno_core@0.399.0",
    "sha256": "<exact unchanged external record SHA-256>"
  }],
  "closure": { "added": [], "changed": ["zfb-js-host@0.0.0"] }
}
```

This is an illustrative, deliberately non-executable template. Derive all hashes
with the exported `parseCargoLock` record parser, and derive `closure.changed`
from the actual receipts; the example is not an approved transition. Commit the
reviewed declaration with the measured candidate. A declaration is proposed
evidence, never approval by itself.

The validator requires:

- Existing source records proven internal by workspace membership, matching
  name/version, `publish = false`, and no lock source/checksum. Only dependency
  additions may change them; added records and dependency removals are refused.
- Exactly declared edges, resolving in both locks to the same external target.
  The target must have an external source and identical complete record bytes;
  alternate spellings of an existing dependency are refused.
- No other changed/added/removed records, header or non-package section changes.
  This preserves the complete external inventory, including checksums and sources.
- Old/new `zfb` normal/build closure receipts for each target and matching lock.
  Both edge endpoints must already be present in both receipts. The command must
  attest the recorder's default-feature `cargo metadata --locked --format-version
  1 --filter-platform <target>` invocation, without custom feature flags. No row
  may leave or join; only explicitly declared internal feature changes may occur.
  Every external row's features and source remain unchanged.
- Existing exact source/platform, artifact, saved-byte, paired declaration and
  transition checks. Any raw or normalized total change, even a decrease, fails.
  Both platforms must agree on declaration bytes, old/new lock hashes and the
  measured source relationship. All prior staging/tamper/rollback checks remain.

Receipt features and membership are operator-attested, not recomputed by the
admission tool. The recorder uses workspace-wide metadata resolution and is not
a substitute for the exact-feature direct-only binary graph. Review the commands
and retain the original receipt bytes; do not hand-edit rows to satisfy the gate.

## Evidence collection handoff (not performed by this extension)

Use isolated old/new checkouts at reviewed exact commits. At the old source the
lock must hash to the currently approved contracts; at the new source it must
match the reviewed declaration and every measured artifact. Record all full SHAs,
clean status, tool versions, binary and artifact hashes, initial failures/retries.

Run these recorder commands at **each** source, changing output names to identify
old/new. Resolution of a Darwin target on Linux is allowed but is metadata only,
never native Mac execution evidence:

```sh
node scripts/record-cargo-producer-closure.mjs --producer zfb \
  --target x86_64-unknown-linux-gnu --output /absolute/evidence/old-or-new-linux.json
node scripts/record-cargo-producer-closure.mjs --producer zfb \
  --target aarch64-apple-darwin --output /absolute/evidence/old-or-new-darwin.json
```

Linux: retain the exact-source ordinary health job's complete
`island-size-linux-x64` artifact, run/job/artifact IDs and hashes. The existing
collection rule can retain unchanged-total evidence when only the old Cargo pin
rejects it. Preserve that rejection; it is neither an admitted lock nor passing CI.

Local Mac: the owner runs the native arm64 collection, independently of this
agent. Follow [the pinned local collection procedure](README.md#pinned-run-after-the-topic-merges)
and the build/pack/measure recipe in `.github/workflows/island-size-darwin.yml`,
using the reviewed candidate SHA, matching pinned toolchain, a fresh default
`cargo build --locked -p zfb`, the shared heavy guard and frozen pnpm install.
Collect both complete eight-fixture workspace/packed matrices, two saved passes
per fixture, tarballs and unpacked packages. Preserve every saved dist inventory,
binary/tarball digest, normalization sidecar and original checker verdict.
Record actual `uname`, `sw_vers`, Rust, Node/zlib, pnpm and esbuild identities.
Retain `runner-provenance.txt` using the validator's field names and **actual**
local values; the expected `runner` platform label is `Darwin arm64`.

After collection, these evidence checks recompute sidecars/check the measurements
(normalization writes only evidence files, never approved contracts):

```sh
node research/v3-island-size/normalize-identity.mjs \
  --measurement /absolute/evidence/real-workspace/measurement.json
node research/v3-island-size/normalize-identity.mjs \
  --measurement /absolute/evidence/real-packed/measurement.json
node research/v3-island-size/check-budget.mjs \
  --contract research/v3-island-size/decision.json \
  --workspace /absolute/evidence/real-workspace/measurement.json \
  --packed /absolute/evidence/real-packed/measurement.json
```

Use `decision-linux-x64.json` for the Linux checker. A stale-pin rejection is
expected until later admission; extra failures or changed totals remain blockers.

**Local evidence ingestion is a separate outstanding boundary:** the existing
Darwin rebaseline CLI requires real Actions run/job/artifact IDs and expiration.
Local native results must remain honestly labeled; never invent those IDs or pass
a local run as an Actions artifact. The separately scoped
[owner-attested local adapter](LOCAL-NATIVE-EVIDENCE.md) adds an explicit receipt
route for `cargo-internal-rewire`. Its integrity checks do not authenticate native
execution; real evidence review and application remain separate owner decisions.

Once native evidence and an honest supported provenance route exist, the existing
[paired staging commands](README.md#cargo-internal-rewire-transitions-4258) take the
schema-2 declaration via `--rewire-declaration`, four receipts via `--old-closure`
and `--new-closure`, and the exact git/lock/source/artifact references. Staging and
application require subsequent authorization. No such command is run here, and
no native Mac Actions dispatch or local Mac task is authorized by this document.
