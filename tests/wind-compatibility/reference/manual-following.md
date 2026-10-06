# Manual upstream-following round: Tailwind 4.3.2 to 4.3.3

This run follows the admitted stable `tailwindcss@4.3.2` reference to the exact
stable `4.3.3` target recorded in
[`manual-following-target.v1.json`](./manual-following-target.v1.json). The
recorded registry scout only selected the target. The repository commands
below independently resolve the exact npm metadata, Git tag, artifact, release
notes, source archive, and version interval before comparison.

Run on the same dedicated Linux CI environment used for required Wind evidence,
with Node 24 and Chromium, Firefox, and WebKit available. Keep one clean
checkout at the tested source SHA for the whole run. Keep plans, artifacts,
reports, browser output, and caches outside the checkout. The accepted
`accepted.json` and `reviewed-through.json` remain the authoritative 4.3.2
records throughout this round.

## Plan and assess the exact target

```sh
EVIDENCE="$(mktemp -d "${RUNNER_TEMP:?}/wind-manual-following.XXXXXX")"
CACHE="$EVIDENCE/cache"
mkdir -p "$CACHE"

node scripts/wind-compatibility/reference-cli.mjs acquire \
  --candidate 4.3.2 --channel stable --live yes --cache "$CACHE" \
  > "$EVIDENCE/accepted-acquisition.json"
node scripts/wind-compatibility/reference-cli.mjs probe \
  --candidate 4.3.2 --channel stable --live yes --cache "$CACHE" \
  --candidates block,hidden,p-0,mx-auto,contents \
  > "$EVIDENCE/accepted-probe.json"
node scripts/wind-compatibility/reference-cli.mjs plan \
  --candidate 4.3.3 --channel stable --live yes > "$EVIDENCE/plan.json"
node scripts/wind-compatibility/reference-cli.mjs acquire \
  --candidate 4.3.3 --channel stable --live yes --cache "$CACHE" \
  > "$EVIDENCE/acquisition.json"
node scripts/wind-compatibility/reference-cli.mjs probe \
  --candidate 4.3.3 --channel stable --live yes --cache "$CACHE" \
  --candidates block,hidden,p-0,mx-auto,contents \
  > "$EVIDENCE/candidate-probe.json"
node scripts/wind-compatibility/reference-cli.mjs assess \
  --plan "$EVIDENCE/plan.json" --cache "$CACHE" \
  > "$EVIDENCE/assessment.json"
node --input-type=module -e '
  import fs from "node:fs";
  import { createHash } from "node:crypto";
  const plan = JSON.parse(fs.readFileSync(process.argv[1]));
  const assessment = JSON.parse(fs.readFileSync(process.argv[2]));
  const acquisition = JSON.parse(fs.readFileSync(process.argv[3]));
  const target = JSON.parse(fs.readFileSync(process.argv[4])).target;
  const cache = process.argv[5];
  const accepted = JSON.parse(fs.readFileSync("tests/wind-compatibility/reference/accepted.json")).acceptedReference;
  const corpus = JSON.parse(fs.readFileSync("tests/wind-compatibility/corpus/manifest.json"));
  if (plan.candidate.version !== "4.3.3" || plan.previousAccepted?.version !== "4.3.2")
    throw Error("Unexpected manual-following version pair");
  if (plan.candidate.integrity !== target.integrity ||
      plan.candidate.tarball !== target.tarball ||
      plan.candidate.sha1 !== target.sha1 ||
      plan.candidate.source?.observedTagCommit !== target.source.observedTagCommit)
    throw Error("Plan differs from the frozen target identity");
  if (assessment.status !== "ready-for-comparison")
    throw Error(`Incomplete upstream assessment: ${assessment.findings.join("; ")}`);
  if (assessment.sections.artifacts.tarballSha256 !== target.preparationAcquisitionSha256 ||
      assessment.sections.source.tagCommit !== target.source.observedTagCommit ||
      assessment.sections.changelog.releaseVersions.join(",") !== "4.3.3")
    throw Error("Assessment differs from the frozen artifact, source, or release identity");
  if (acquisition.version !== target.version || acquisition.integrity !== target.integrity ||
      acquisition.sha256 !== target.preparationAcquisitionSha256)
    throw Error("Acquisition differs from the frozen target identity");
  const sourceArchive = `${cache}/response-${corpus.sourceArchiveSha256}`;
  const acceptedTarball = `${cache}/response-${accepted.artifactSha256}`;
  const acceptedCompiler = `${cache}/tailwindcss-${accepted.version}-${accepted.artifactSha256}/dist/lib.mjs`;
  for (const path of [sourceArchive, acceptedTarball, acceptedCompiler])
    if (!fs.statSync(path).isFile()) throw Error(`Pinned comparison input missing: ${path}`);
  const acceptedHash = createHash("sha256").update(fs.readFileSync(acceptedTarball)).digest("hex");
  if (acceptedHash !== accepted.artifactSha256)
    throw Error("Accepted 4.3.2 artifact digest changed");
  const sourceHash = createHash("sha256").update(fs.readFileSync(sourceArchive)).digest("hex");
  if (sourceHash !== corpus.sourceArchiveSha256)
    throw Error("Pinned corpus source archive digest changed");
' "$EVIDENCE/plan.json" "$EVIDENCE/assessment.json" \
  "$EVIDENCE/acquisition.json" tests/wind-compatibility/reference/manual-following-target.v1.json "$CACHE"
```

The commands use one fresh cache for the admitted 4.3.2 compiler, candidate
4.3.3 compiler, and the pinned corpus source archive. The presence and hashes
of the accepted tarball and corpus archive are checked before comparison; the
repository validator replays the extracted compiler-module identities.

Stop if assessment is incomplete. It must retain the complete release-note
interval, artifact file delta, source file delta, test file delta, captured
response digests, exact candidate SRI and tarball digest, and the explicit
unverified tag-to-artifact relationship. Do not replace missing source or
release evidence with a registry scout, a guessed version range, or a partial
archive.

## Run the same Wind comparison against both references

Build one Wind executable and manifest from the clean tested source, then use
that exact pair for the accepted and candidate compilers:

```sh
node scripts/wind-compatibility/differential-runner.mjs build-wind \
  "$EVIDENCE/wind-build.json"
WIND_BINARY="$(node -p 'JSON.parse(require("node:fs").readFileSync(process.argv[1])).binaryPath' \
  "$EVIDENCE/wind-build.json")"
node scripts/wind-compatibility/reference-cli.mjs compare \
  --plan "$EVIDENCE/plan.json" \
  --assessment "$EVIDENCE/assessment.json" \
  --cache "$CACHE" \
  --wind-binary "$WIND_BINARY" \
  --wind-build-manifest "$EVIDENCE/wind-build.json" \
  --output "$EVIDENCE/comparison"
```

`compare` executes the full declared pilot and corpus in each required browser
engine. It uses the same profile, case membership, configuration, browser
builds, and Wind executable for accepted 4.3.2 and candidate 4.3.3, and retains
the raw CSS, observations, browser identities, and per-case deltas. The
comparison must report refreshed candidate inventory membership. Every
accepted-baseline case must pass. Candidate failures are retained as observed
mismatches; they are not candidate parity. Missing rows or browser/build
identity differences invalidate the run.

## Prepare and review a conservative deferral

Create an exact-membership proposal from the report:

```sh
node scripts/wind-compatibility/reference-rehearsal.mjs propose \
  --evidence "$EVIDENCE"
```

The proposal copies each changed upstream, browser, control, Wind, and inventory
row with its observed values. It labels each as `unexplained-drift` and
`review-only`; this is a conservative machine-generated proposal, not semantic
review. Do not infer a source-to-npm artifact link, adopt utilities
automatically, or change implementation, fixtures, pins, or assertions in this
round.

Run the disposable checked-transition rehearsal with that proposal:

```sh
node scripts/wind-compatibility/reference-rehearsal.mjs rehearse \
  --evidence "$EVIDENCE" --cache "$CACHE"
```

The helper requires a clean checkout, exact 4.3.2 to 4.3.3 plan, complete
assessment, accepted-baseline pass, candidate inventory refresh, and a
review-only classification proposal. It invokes `reference-cli.mjs promote`
first as a dry run and then as a checked apply in repository metadata, captures
the proposed pair outside the checkout, and restores the original state bytes
in all outcomes. Its summary marks `authoritative: false` and
`reviewStatus: requires-independent-evidence-review-before-commit`. It
verifies dry-run/apply identity and unchanged report bytes. The accepted record
must remain 4.3.2; only the disposable proposed `reviewed-through` record may
reach 4.3.3. This does not publish, commit, push, or post externally.

An independent evidence reviewer must inspect the full retained rows and raw
outputs before any authoritative metadata change. If the manager accepts the
review-only proposal, verify the artifact's dry-run and applied outputs,
`rehearsal-summary.json`, and state hashes. Compare the artifact's
`state/accepted.json` byte-for-byte with the current accepted record, then copy
only the artifact's `state/reviewed-through.json` to the tracked path. Confirm
that it names plan, assessment, comparison, classification, source, and input
identities from the reviewed artifacts and has `disposition: review-only`.
Commit only `reviewed-through.json`; keep `accepted.json` byte-for-byte
unchanged. The disposable proposal and its summary do not constitute
independent approval. Candidate acceptance requires a separate complete
eligibility and shipping review.

If the independent review changes the proposed classification, rerun a checked
transition on a compatible Linux Node runtime with the exact original source
and comparison artifacts. Do not hand-edit the generated state record or claim
that a local replay on another runtime validated those artifacts.

Retain the plan, acquisition, assessment, comparison output tree, proposed
classification, CLI outputs, state pair, cache hashes, and
`rehearsal-summary.json` as the execution evidence. The summary records the
candidate pass result, exact report identities, tested source/input identity,
inventory status, restoration checks, and the independent-review boundary. A
final result must cite those artifacts and must not claim acceptance unless a
separate fully eligible acceptance workflow is reviewed and deliberately
selected.
