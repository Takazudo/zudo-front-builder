#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { testedInputIdentity } from "./reference-identity.mjs";
import { digest, fromRoot, outsideCheckout, paths, readJson, sha256 } from "./reference.mjs";

const DELTA_ROWS = {
  upstreamDelta: 112,
  browserDelta: 358,
  controlDelta: 33,
  windDelta: 189,
};
const REPORTS = [
  "corpus-chromium",
  "corpus-firefox",
  "corpus-webkit",
  "pilot-chromium",
  "pilot-firefox",
  "pilot-webkit",
];
const CHANGE_ARRAYS = [
  "upstreamChanges",
  "browserChanges",
  "controlChanges",
  "windChanges",
  "inventoryChanges",
];
const PROFILE = { id: "wind-preset-free", version: 1, revision: 5 };
const CANDIDATE_VERSION = "4.3.2";
const CLASSIFICATION_POLICY = "wind-first-reference-empty-baseline-v1";
const CLI = fromRoot("scripts/wind-compatibility/reference-cli.mjs");

function requireThat(condition, message) {
  if (!condition) throw Error(message);
}

function objectId(value, key) {
  const body = Object.fromEntries(Object.entries(value ?? {}).filter(([name]) => name !== key));
  return `sha256:${digest(body)}`;
}

/**
 * Build the only classification permitted for the first reference bootstrap.
 * Full report/artifact replay and shipping validation remain the CLI's job.
 */
export function buildBootstrapClassification({
  profile,
  accepted,
  reviewed,
  plan,
  assessment,
  comparison,
  sourceSha,
  testedInputsDigest,
}) {
  requireThat(
    profile?.schemaVersion === 1 &&
      profile.profileId === PROFILE.id &&
      profile.profileVersion === PROFILE.version &&
      profile.profileRevision === PROFILE.revision &&
      profile.referencePolicy?.initialCandidateVersion === CANDIDATE_VERSION &&
      profile.referencePolicy?.initialState?.acceptedReference === null &&
      profile.referencePolicy?.initialState?.reviewedThrough === null,
    "Unknown profile or changed bootstrap policy",
  );
  requireThat(
    accepted?.schemaVersion === 1 &&
      accepted.acceptedReference === null &&
      reviewed?.schemaVersion === 1 &&
      reviewed.reviewedThrough === null,
    "Bootstrap requires both live reference state records to be null",
  );
  requireThat(
    plan?.schemaVersion === 1 &&
      plan.kind === "wind-reference-plan" &&
      !Object.hasOwn(plan, "verificationOnly") &&
      plan.planId === objectId(plan, "planId") &&
      plan.channel === "stable" &&
      plan.candidate?.package === "tailwindcss" &&
      plan.candidate.version === CANDIDATE_VERSION &&
      plan.previousAccepted === null &&
      plan.previousReviewedThrough === null &&
      plan.changes?.bootstrap === true &&
      plan.profile?.id === PROFILE.id &&
      plan.profile.version === PROFILE.version &&
      plan.profile.revision === PROFILE.revision,
    "Plan is not the exact first stable bootstrap",
  );
  requireThat(
    assessment?.schemaVersion === 1 &&
      assessment.kind === "wind-reference-assessment" &&
      assessment.status === "ready-for-comparison" &&
      assessment.planId === plan.planId &&
      assessment.verificationOnly !== true &&
      /^sha256:[0-9a-f]{64}$/.test(assessment.captureIdentity ?? ""),
    "Assessment is missing, stale, or not for this bootstrap plan",
  );
  requireThat(
    comparison?.schemaVersion === 1 &&
      comparison.kind === "wind-three-way-comparison" &&
      comparison.reportId === objectId(comparison, "reportId") &&
      comparison.planId === plan.planId &&
      comparison.assessmentIdentity === assessment.captureIdentity &&
      comparison.testedSourceSha === sourceSha &&
      comparison.testedInputs?.schemaVersion === 2 &&
      comparison.testedInputs.digest === testedInputsDigest &&
      comparison.profile?.id === PROFILE.id &&
      comparison.profile.version === PROFILE.version &&
      comparison.profile.revision === PROFILE.revision &&
      comparison.profile.digest === plan.inputs?.profile &&
      comparison.admission === "pending-independent-review-and-shipping-evidence" &&
      comparison.candidate?.passing === true &&
      comparison.candidate.windBuild?.gitSha === sourceSha &&
      comparison.candidate.reference?.version === CANDIDATE_VERSION &&
      comparison.accepted == null &&
      comparison.candidateInventory === null &&
      comparison.inventoryMembership === "bootstrap-pinned-inventory" &&
      digest(Object.keys(comparison.candidate.reports ?? {}).sort()) === digest(REPORTS),
    "Comparison is stale, incomplete, non-bootstrap, or failed",
  );
  requireThat(
    /^[0-9a-f]{40}$/.test(sourceSha ?? "") &&
      /^sha256:[0-9a-f]{64}$/.test(testedInputsDigest ?? ""),
    "Current source or tested-input identity is malformed",
  );

  for (const [name, expectedCount] of Object.entries(DELTA_ROWS)) {
    const rows = comparison[name];
    requireThat(
      Array.isArray(rows) && rows.length === expectedCount,
      `Unexpected ${name} membership; expected ${expectedCount} rows`,
    );
    for (const row of rows) {
      requireThat(
        row && typeof row === "object" && row.changed === null,
        `Unexpected ${name} change flag; bootstrap rows must be null`,
      );
      if (name === "browserDelta")
        requireThat(
          row.localChanged === null,
          "Unexpected browser localChanged flag; bootstrap rows must be null",
        );
    }
  }

  return {
    schemaVersion: 1,
    kind: "wind-reference-classification",
    bootstrapPolicy: CLASSIFICATION_POLICY,
    planId: plan.planId,
    comparisonReportId: comparison.reportId,
    disposition: "accept",
    ...Object.fromEntries(CHANGE_ARRAYS.map((name) => [name, []])),
  };
}

function parseArgs(argv) {
  const [command, ...args] = argv;
  if (command !== "bootstrap")
    throw Error(
      "Usage: bootstrap-admission.mjs bootstrap --evidence /outside/checkout/wind-gate --cache /outside/checkout/wind-reference-cache",
    );
  const options = {};
  for (let index = 0; index < args.length; index += 2) {
    const name = args[index];
    if (!name?.startsWith("--") || !args[index + 1] || args[index + 1].startsWith("--"))
      throw Error(`Expected --key value, got ${name}`);
    const key = name.slice(2);
    if (!new Set(["evidence", "cache"]).has(key) || options[key] !== undefined)
      throw Error(`Unknown or duplicate option ${name}`);
    options[key] = args[index + 1];
  }
  if (!options.evidence || !options.cache) throw Error("bootstrap requires --evidence and --cache");
  return options;
}

function runCli(args) {
  try {
    return execFileSync(process.execPath, [CLI, ...args], {
      cwd: fromRoot("."),
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      maxBuffer: 16 * 1024 * 1024,
    });
  } catch (error) {
    const stderr = error.stderr?.toString("utf8").trim();
    throw Error(stderr || error.message);
  }
}

async function snapshotTree(root) {
  const rows = [];
  async function visit(directory, prefix = "") {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) await visit(path, relative);
      else if (entry.isFile()) rows.push([relative, sha256(await readFile(path))]);
      else throw Error(`Unsupported report artifact type: ${relative}`);
    }
  }
  await visit(root);
  return digest(rows);
}

async function writeNew(path, bytes) {
  await writeFile(path, bytes, { flag: "wx" });
}

function cliArgs({
  planPath,
  assessmentPath,
  cache,
  comparisonPath,
  classificationPath,
  shippingPath,
  shippingOutput,
}) {
  return [
    "promote",
    "--plan",
    planPath,
    "--assessment",
    assessmentPath,
    "--cache",
    cache,
    "--output",
    comparisonPath,
    "--classification",
    classificationPath,
    "--shipping",
    shippingPath,
    "--shipping-output",
    shippingOutput,
  ];
}

function validateTransitionResult(
  result,
  { dryRun, classification, comparison, sourceSha, inputsDigest },
) {
  const accepted = result.next?.accepted?.acceptedReference;
  const reviewed = result.next?.reviewed?.reviewedThrough;
  requireThat(result.dryRun === dryRun, "CLI transition run mode changed");
  requireThat(
    result.testedInputs?.schemaVersion === 2 && result.testedInputs.digest === inputsDigest,
    "CLI transition tested-input identity changed",
  );
  requireThat(
    accepted?.version === CANDIDATE_VERSION &&
      accepted.package === "tailwindcss" &&
      accepted.channel === "stable" &&
      accepted.evidenceReportId === comparison.reportId &&
      accepted.testedSourceSha === sourceSha &&
      accepted.finalVerificationSha === sourceSha &&
      accepted.testedInputsDigest === inputsDigest &&
      accepted.shippingEvidenceDigest &&
      accepted.shippingManifestDigest,
    "CLI did not produce the expected accepted reference identity",
  );
  requireThat(
    reviewed?.version === CANDIDATE_VERSION &&
      reviewed.package === "tailwindcss" &&
      reviewed.disposition === "accept" &&
      reviewed.planId === comparison.planId &&
      reviewed.comparisonReportId === comparison.reportId &&
      reviewed.classificationDigest === `sha256:${digest(classification)}` &&
      reviewed.testedSourceSha === sourceSha &&
      reviewed.finalVerificationSha === sourceSha &&
      reviewed.testedInputsDigest === inputsDigest,
    "CLI did not produce the expected reviewed-through identity",
  );
}

/** Run the CLI transition, preserve its state pair, then restore original state bytes. */
export async function runEphemeralPromotion({
  acceptedPath,
  reviewedPath,
  runDry,
  runApply,
  recover,
  validateDry,
  validateApplied,
  writeDryOutput,
  writeAppliedOutput,
  writeStateArtifact,
}) {
  const originalAccepted = await readFile(acceptedPath);
  const originalReviewed = await readFile(reviewedPath);
  let operationError;
  let result;
  let applyStarted = false;

  try {
    const dryRaw = await runDry();
    const dry = JSON.parse(dryRaw);
    validateDry(dry);
    await writeDryOutput(dryRaw);

    applyStarted = true;
    const appliedRaw = await runApply();
    const applied = JSON.parse(appliedRaw);
    validateApplied(applied);
    requireThat(
      digest(dry.next) === digest(applied.next) &&
        digest(dry.testedInputs) === digest(applied.testedInputs),
      "Checked apply differs from its dry-run proposal",
    );
    await writeAppliedOutput(appliedRaw);

    const acceptedBytes = await readFile(acceptedPath);
    const reviewedBytes = await readFile(reviewedPath);
    requireThat(
      digest(JSON.parse(acceptedBytes.toString("utf8"))) === digest(applied.next.accepted) &&
        digest(JSON.parse(reviewedBytes.toString("utf8"))) === digest(applied.next.reviewed),
      "Written state pair differs from the checked CLI output",
    );
    result = { dry, applied, state: { accepted: acceptedBytes, reviewed: reviewedBytes } };
    await writeStateArtifact(result.state);
  } catch (error) {
    operationError = error;
  } finally {
    const cleanupErrors = [];
    let stateChanged = false;
    try {
      stateChanged =
        !(await readFile(acceptedPath)).equals(originalAccepted) ||
        !(await readFile(reviewedPath)).equals(originalReviewed);
    } catch (error) {
      stateChanged = true;
      cleanupErrors.push(error);
    }
    if (applyStarted || stateChanged) {
      try {
        await recover();
      } catch (error) {
        cleanupErrors.push(error);
      }
    }
    try {
      const currentAccepted = await readFile(acceptedPath).catch(() => null);
      const currentReviewed = await readFile(reviewedPath).catch(() => null);
      if (!currentAccepted?.equals(originalAccepted))
        await writeFile(acceptedPath, originalAccepted);
      if (!currentReviewed?.equals(originalReviewed))
        await writeFile(reviewedPath, originalReviewed);
      const restoredAccepted = await readFile(acceptedPath);
      const restoredReviewed = await readFile(reviewedPath);
      requireThat(
        restoredAccepted.equals(originalAccepted) && restoredReviewed.equals(originalReviewed),
        "Ephemeral bootstrap did not restore exact original state bytes",
      );
    } catch (error) {
      cleanupErrors.push(error);
    }
    if (cleanupErrors.length) {
      if (operationError)
        throw new AggregateError(
          [operationError, ...cleanupErrors],
          "Bootstrap failed and ephemeral state cleanup reported an error",
        );
      throw cleanupErrors[0];
    }
  }

  if (operationError) throw operationError;
  return result;
}

async function main(argv) {
  const options = parseArgs(argv);
  const evidence = await outsideCheckout(resolve(options.evidence));
  const cache = await outsideCheckout(resolve(options.cache));
  const admission = resolve(evidence, "admission");
  const acceptedPath = fromRoot(paths.accepted);
  const reviewedPath = fromRoot(paths.reviewed);
  const dirty = execFileSync("git", ["status", "--porcelain", "--untracked-files=all"], {
    cwd: fromRoot("."),
    encoding: "utf8",
  });
  requireThat(!dirty, "Bootstrap transition requires a clean source checkout");
  await mkdir(admission);

  const planPath = resolve(evidence, "plan.json");
  const assessmentPath = resolve(evidence, "assessment.json");
  const comparisonPath = resolve(evidence, "comparison");
  const comparisonFile = resolve(comparisonPath, "comparison.json");
  const shippingOutput = resolve(evidence, "shipping");
  const shippingPath = resolve(shippingOutput, "report.json");
  const classificationPath = resolve(admission, "classification.json");
  const stateOutput = resolve(admission, "state");
  const dryPath = resolve(admission, "dry-run.json");
  const appliedPath = resolve(admission, "applied.json");
  const summaryPath = resolve(admission, "summary.json");
  const reportPaths = [planPath, assessmentPath, comparisonFile, shippingPath];
  const reportBytes = await Promise.all(reportPaths.map((path) => readFile(path)));
  const comparisonTree = await snapshotTree(comparisonPath);
  const shippingTree = await snapshotTree(shippingOutput);
  const profile = await readJson(fromRoot(paths.profile));
  const accepted = await readJson(acceptedPath);
  const reviewed = await readJson(reviewedPath);
  const plan = await readJson(planPath);
  const assessment = await readJson(assessmentPath);
  const comparison = await readJson(comparisonFile);
  const sourceSha = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: fromRoot("."),
    encoding: "utf8",
  }).trim();
  const inputs = await testedInputIdentity();
  const classification = buildBootstrapClassification({
    profile,
    accepted,
    reviewed,
    plan,
    assessment,
    comparison,
    sourceSha,
    testedInputsDigest: inputs.digest,
  });
  await writeNew(classificationPath, `${JSON.stringify(classification, null, 2)}\n`);

  const common = cliArgs({
    planPath,
    assessmentPath,
    cache,
    comparisonPath,
    classificationPath,
    shippingPath,
    shippingOutput,
  });
  const transition = await runEphemeralPromotion({
    acceptedPath,
    reviewedPath,
    runDry: () => runCli(common),
    runApply: () => runCli([...common, "--apply", "yes"]),
    recover: () => runCli(["recover"]),
    validateDry: (result) =>
      validateTransitionResult(result, {
        dryRun: true,
        classification,
        comparison,
        sourceSha,
        inputsDigest: inputs.digest,
      }),
    validateApplied: (result) =>
      validateTransitionResult(result, {
        dryRun: false,
        classification,
        comparison,
        sourceSha,
        inputsDigest: inputs.digest,
      }),
    writeDryOutput: (raw) => writeNew(dryPath, raw),
    writeAppliedOutput: (raw) => writeNew(appliedPath, raw),
    writeStateArtifact: async ({ accepted: acceptedBytes, reviewed: reviewedBytes }) => {
      await mkdir(stateOutput);
      await writeNew(resolve(stateOutput, "accepted.json"), acceptedBytes);
      await writeNew(resolve(stateOutput, "reviewed-through.json"), reviewedBytes);
    },
  });

  for (let index = 0; index < reportPaths.length; index++) {
    const current = await readFile(reportPaths[index]);
    requireThat(
      current.equals(reportBytes[index]),
      `Original report bytes changed: ${reportPaths[index]}`,
    );
  }
  requireThat(
    (await snapshotTree(comparisonPath)) === comparisonTree &&
      (await snapshotTree(shippingOutput)) === shippingTree,
    "Original comparison or shipping artifacts changed",
  );

  const summary = {
    schemaVersion: 1,
    kind: "wind-bootstrap-transition-artifact",
    authoritative: false,
    sourceSha,
    planId: plan.planId,
    assessmentIdentity: assessment.captureIdentity,
    comparisonReportId: comparison.reportId,
    shippingReportId: (await readJson(shippingPath)).reportId,
    testedInputsDigest: inputs.digest,
    candidate: { package: "tailwindcss", version: CANDIDATE_VERSION, channel: "stable" },
    profile: PROFILE,
    deltaRows: DELTA_ROWS,
    changedRows: { upstream: 0, browser: 0, control: 0, wind: 0 },
    classificationPolicy: CLASSIFICATION_POLICY,
    classificationDigest: `sha256:${digest(classification)}`,
    nextStateDigest: `sha256:${digest(transition.applied.next)}`,
    reportBytesPreserved: true,
    stateFiles: {
      acceptedSha256: sha256(transition.state.accepted),
      reviewedThroughSha256: sha256(transition.state.reviewed),
    },
    reviewStatus: "requires-independent-evidence-review-before-commit",
  };
  await writeNew(summaryPath, `${JSON.stringify(summary, null, 2)}\n`);

  const dirtyAfter = execFileSync("git", ["status", "--porcelain", "--untracked-files=all"], {
    cwd: fromRoot("."),
    encoding: "utf8",
  });
  requireThat(!dirtyAfter, "Ephemeral bootstrap left the checkout dirty");
  process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).catch((error) => {
    const messages = [error.message];
    if (error instanceof AggregateError)
      for (const nested of error.errors) messages.push(nested.stack ?? nested.message);
    process.stderr.write(`${messages.join("\n")}\n`);
    process.exitCode = 1;
  });
}
