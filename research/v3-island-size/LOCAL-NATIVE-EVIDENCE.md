# Owner-attested local Darwin evidence

This adapter extends `cargo-internal-rewire`. It accepts retained native Darwin
arm64 bundles through the same measurement, normalization, fingerprint, Cargo
closure/declaration and paired-stage validators as Actions artifacts. It changes no
ceiling, allowance, budget pin, default feature, producer or collection command.
Other reasons still require their existing provenance route.

## One module, two reviewed profiles

`scripts/island-size-local-evidence.mjs` holds exactly two local-evidence
profiles in `LOCAL_EVIDENCE_PROFILES`, each for its own purpose:

| Reason | Purpose | Transition binding | Lock binding |
| --- | --- | --- | --- |
| `cargo-internal-rewire` | #4311 internal Cargo rewires and existing external edges | `baselineEvidence.cargoRewire` from the existing rewire validators | Whatever those validators admit; no extra pin |
| `cargo-producer-lock` | #4314 rustls security transition ([recipe](LOCAL-SECURITY-4314.md)) | `baselineEvidence.cargoTransition` equal to the exact `rustls-security-4314` decision | Exact old/new `Cargo.lock` SHA-256 of that decision |

A stage whose evidence carries the other profile's transition key is rejected,
and so is any reason not in the table. Adding a reason is a provenance-policy
decision for the owner, not a refactor. Both profiles share the same receipt,
bundle digest, staging-only CLI and tooling-identity rules below.

Development parent: `base/4314-security-darwin-evidence` (unified with #4319);
originally draft #4310 (now merged to `main`). The dependency DAG is this adapter
→ separately authorized evidence application. The manager owns focused regression tests, independent review and
exact-head CI under
[the repository integration policy](../../CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions).
This implementation does not authorize native collection, contract application,
merge or release. #4307 and the owner's #4308 Mac work remain separate.

## Trust and receipt

The owner attests who ran the commands and where. SHA-256 binds retained bytes;
it does not authenticate the operator, prove the host identity, or prove execution.
A self-consistent invented receipt is not distinguishable from an honest receipt
by hashes alone. There is no signing policy and no invented Actions identifier.
Review the receipt and its expected digest out of band before using them.

Keep the bundle, receipt and stage outside the source checkout. The checkout must
be at the exact measured commit and clean: tracked changes and untracked files
are rejected; ignored build outputs are permitted. The receipt is a regular file
outside the bundle. Symlinks in either path (including ancestor directories),
symlinks inside the bundle, special files and unsafe entry names are rejected.
On macOS use canonical physical paths rather than `/tmp` or other symlink aliases.

The receipt has exactly these fields (replace illustrative values):

```json
{
  "schemaVersion": 1,
  "kind": "local-native-darwin",
  "operator": "owner identity",
  "collectedAt": "2026-10-18T12:00:00.000Z",
  "source": { "commit": "FULL_40_CHARACTER_COMMIT", "clean": true },
  "platform": { "os": "darwin", "arch": "arm64" },
  "commands": [["actual-command", "actual-argument"]],
  "bundle": {
    "location": "retained archive location",
    "sha256": "FULL_64_CHARACTER_BUNDLE_DIGEST"
  }
}
```

Record the actual command argument arrays; the adapter never executes them.
Collection time must be a valid past UTC timestamp including milliseconds.
`bundle.location` describes the retained archive; it is not fetched or executed.
The actual input directory is supplied separately via `--artifact`.

Compute the bundle digest with the exported `localBundleDigest(directory)` from
`scripts/island-size-local-evidence.mjs`. It hashes the JSON serialization of the
complete recursively sorted inventory of `[relativePath, byteLength, sha256]`
records. Paths use `/`; traversal sorts each directory's names. Empty directories
carry no evidence; empty bundles are rejected. Nothing is excluded. Write the
receipt outside that directory and separately compute SHA-256 of its exact bytes.
Retain both the receipt and bundle unchanged through staging and application.

## Staging interface

Use the existing Darwin `cargo-internal-rewire` arguments and paired procedure,
substituting these local arguments for **all four** Actions fields (`--run-id`,
`--job-id`, `--artifact-id`, `--artifact-expires-at`):

```text
--artifact /absolute/retained-bundle
--source-root /absolute/measured-checkout
--source-sha <exact-measured-checkout-HEAD>
--tool-sha <exact-HEAD-of-the-tooling-checkout-running-the-CLI>
--local-receipt /absolute/receipt.json
--local-receipt-sha <reviewed-SHA256-of-receipt-bytes>
--reason cargo-internal-rewire
--stage /absolute/paired-stage
```

`--stage` and `--tool-sha` are mandatory for local evidence. The CLI may run from
a separate clean tooling checkout; both that checkout (`--tool-sha`) and the
measured checkout (`--source-sha`) must be clean at their exact commits, and the
stage records both identities (`stagingTool` names the renderer only; it is not
approval for a later apply). Without `--source-root`, the measured checkout is
the current directory.

The old/new Cargo hashes, old git ref, reviewed declaration, target-specific
closure receipts and predecessor checks remain required. See
[the existing external-edge handoff](EXISTING-EXTERNAL-EDGES.md). Both declaration
schemas are supported; this does not expand which Cargo transitions are allowed.
The Linux side continues to use its existing Actions provenance and must describe
the same candidate through its source/head binding. Either staging order works.

The rendered Darwin contract records `event: "local-native"` and
`localReceipt: { sha256, receipt }` plus `stagingTool: { commit }`. It has no
Actions run/job/artifact identity. The stage binds the canonical absolute receipt
and bundle paths, receipt hash and tooling checkout. On extension or application,
these bytes, the source and tooling identities and both clean states are checked
again before writes. Removing the local binding, mixing provenance, relabelling
the stage to the other profile, changing retained files or dropping the matching
Darwin run fails closed. Existing
stage file hashes, paired declaration checks, live-file checks and application
rollback remain in force. As with existing stages, this is an integrity boundary,
not a signed authorization boundary against someone who can replace every input.

Application remains the existing `--apply <paired-stage>` operation, only after
separate owner authorization. Local receipt flags are invalid on `--apply`, the
fingerprint helper, other reasons and the Linux CLI. No live contract is applied
as part of implementing or testing this adapter.

## Evidence limits

Linux fixture tests exercise positive paired application and negative controls
for altered receipt/bundle/source/binding/clean state, tooling drift, a stage
relabelled to the other profile, corrupt, swapped and wrong-SHA receipts and
bundles, invalid native platforms, malformed receipts, mixed Actions fields and
symlink escapes. Existing Actions
and paired rollback tests remain regression gates. Native Darwin execution must
still be supplied by the owner's independent collection; test fixtures are not
Mac evidence. The current exact-head CI result belongs in the draft PR, not in a
claim that a real producer has already been admitted.
