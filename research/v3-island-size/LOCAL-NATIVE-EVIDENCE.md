# Owner-attested local Darwin evidence

This adapter extends `cargo-internal-rewire`, `cargo-removal`, the exact #4314 `cargo-producer-lock`
transition, the provenance-only `release-bump` and `runtime-identity` re-pins and the
paired `dependency-graph` re-pin (see the profile table below). It accepts retained native Darwin
arm64 bundles through the same measurement, normalization, fingerprint, Cargo
closure/declaration and paired-stage validators as Actions artifacts. It changes no
ceiling, allowance, budget pin, default feature, producer or collection command.
Every other reason still requires its existing provenance route.

## One module, six reviewed profiles

`scripts/island-size-local-evidence.mjs` holds exactly six local-evidence
profiles in `LOCAL_EVIDENCE_PROFILES`, each for its own purpose:

| Reason | Purpose | Transition binding | Lock binding |
| --- | --- | --- | --- |
| `cargo-internal-rewire` | #4311 internal Cargo rewires and existing external edges | `baselineEvidence.cargoRewire` from the existing rewire validators | Whatever those validators admit; no extra pin |
| `cargo-removal` | #4388 removal-only Cargo transitions (the Deno exit) | `baselineEvidence.cargoRemoval` from the removal validators; `outcome` must be `cargo-removal-transition` | Whatever those validators admit; no extra pin |
| `cargo-producer-lock` | #4314 rustls security transition ([recipe](LOCAL-SECURITY-4314.md)) | `baselineEvidence.cargoTransition` equal to the exact `rustls-security-4314` decision | Exact old/new `Cargo.lock` SHA-256 of that decision |
| `release-bump` | Darwin package-version re-pin after a release (owner decision 2026-10-21) | None; `outcome` must be `release-version-stamp` | `Cargo.lock` must not move (the re-pin's own refusal) |
| `runtime-identity` | Darwin runtime-identity re-pin (same decision) | None; `outcome` must be `runtime-identity-repin` | `Cargo.lock` must not move (the re-pin's own refusal) |
| `dependency-graph` | Darwin half of the paired #4218 dependency-graph re-pin (owner decision 2026-10-28, #4580) | `baselineEvidence.fingerprintTransition` from the shared fingerprint refusals; `outcome` must be `dependency-graph-repin` | `Cargo.lock` must not move (the re-pin's own refusal) |

On 2026-10-21 the owner decided that Darwin re-pins must not require a hosted
Mac runner, so the two provenance-only re-pins joined the table. They keep every
Actions-route check: equal raw and `gzipNormalized` totals, zero allowance, no
`Cargo.lock` or dependency-fingerprint movement, and the checkout identity match.
Only the evidence source changes. They are not paired: each stages Darwin alone
and is applied from a clean checkout at the measured source.

On 2026-10-28 the owner approved the zudo-doc 9.1.0 docs bump (#4580), which
moves only the dependency fingerprint, through local Darwin evidence, so
`dependency-graph` joined the table. It keeps every check of the Actions route:
only `dependencyFingerprint.sha256` and the recorded `pnpm-lock.yaml` hash may
move, every raw and `gzipNormalized` total stays equal, and the package version,
`Cargo.lock`, fixtures, runners and normalizer must match the contract. It stays
paired: the Linux half still comes from the `health` artifact of the same
candidate, both stage into one directory, and `--apply` lands them together.

A stage whose evidence carries another profile's transition key is rejected,
and so is any reason not in the table. Adding a reason is a provenance-policy
decision for the owner, not a refactor. All profiles share the same receipt,
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

Use the existing Darwin `cargo-internal-rewire` (or `cargo-removal`) arguments and paired procedure,
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
the current directory. Relative input file paths (`--old-cargo-lock`,
`--rewire-declaration`, `--removal-declaration`, `--old-closure`, `--new-closure`,
`--reviewed-cargo-transition`) are read from the measured checkout. For the
security profile, use [LOCAL-SECURITY-4314.md](LOCAL-SECURITY-4314.md) instead;
only that profile requires `--reviewed-cargo-transition`.

For `dependency-graph`, pass the same receipt, tooling and source flags with
`--reason dependency-graph` and the `--stage` directory the Linux run stages
into (either order). `--source-sha` must be the Linux run's PR merge source or
its probe head, as on the Actions route. No Cargo flag is accepted.

For `release-bump` or `runtime-identity`, pass the same receipt, tooling and
source flags with `--reason release-bump` (or `runtime-identity`), an
unpaired `--stage` directory and `--previous-contract-ref <measured-source>`.
No Cargo flag is accepted, because neither reason may move `Cargo.lock`. Apply
the stage with `--apply` from a clean checkout at the measured source; the
tooling checkout recorded in the stage must also still be clean at `--tool-sha`.

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
