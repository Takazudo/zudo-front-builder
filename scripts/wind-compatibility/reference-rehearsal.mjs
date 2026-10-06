#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { runEphemeralPromotion } from "./bootstrap-admission.mjs";
import { testedInputIdentity } from "./reference-identity.mjs";
import { digest, fromRoot, outsideCheckout, paths, readJson, sha256 } from "./reference.mjs";

const CLI = fromRoot("scripts/wind-compatibility/reference-cli.mjs");
const ACCEPTED_VERSION = "4.3.2";
const CANDIDATE_VERSION = "4.3.3";
const CHANGE_CATEGORIES = new Set([
  "upstream-semantic",
  "upstream-default-theme",
  "upstream-packaging",
  "intentional-exclusion",
  "local-implementation-gap",
  "unexplained-drift",
]);

function requireThat(condition, message) {
  if (!condition) throw Error(message);
}

function objectId(value, key) {
  const body = Object.fromEntries(Object.entries(value ?? {}).filter(([name]) => name !== key));
  return `sha256:${digest(body)}`;
}

function rationale(subject, accepted, candidate) {
  return `${subject}: accepted ${digest(accepted)}, candidate ${digest(candidate)}; semantic disposition remains independently unreviewed.`;
}

function inventoryChanges(comparison) {
  const changes = [];
  for (const [source, domains] of Object.entries({
    runtime: comparison.candidateInventory?.runtimeChanges ?? {},
    source: comparison.candidateInventory?.sourceChanges ?? {},
  }))
    for (const [domain, actions] of Object.entries(domains))
      for (const action of ["added", "removed"])
        for (const value of actions[action] ?? []) changes.push({ source, domain, action, value });
  for (const [domain, value] of Object.entries(
    comparison.candidateInventory?.sourceMetadataChanges ?? {},
  ))
    changes.push({ source: "source-metadata", domain, action: "changed", value });
  return changes;
}

export function validateFrozenTarget({ plan, assessment, acquisition, targetRecord }) {
  const target = targetRecord?.target,
    accepted = targetRecord?.currentAccepted;
  requireThat(
    targetRecord?.schemaVersion === 1 &&
      targetRecord.kind === "wind-manual-following-target" &&
      accepted?.package === "tailwindcss" &&
      accepted?.version === ACCEPTED_VERSION &&
      accepted.channel === "stable" &&
      plan?.previousAccepted?.version === accepted.version &&
      plan.previousAccepted.package === accepted.package &&
      plan.previousAccepted.channel === accepted.channel &&
      plan.previousAccepted.integrity === accepted.integrity &&
      plan.previousAccepted.artifactSha256 === accepted.artifactSha256 &&
      target?.package === "tailwindcss" &&
      target.version === CANDIDATE_VERSION &&
      target.channel === "stable" &&
      plan?.candidate?.package === target.package &&
      plan.candidate.version === target.version &&
      plan.candidate.integrity === target.integrity &&
      plan.candidate.tarball === target.tarball &&
      plan.candidate.sha1 === target.sha1 &&
      target.registry === `https://registry.npmjs.org/${target.package}/${target.version}` &&
      target.source?.repository === "https://github.com/tailwindlabs/tailwindcss.git" &&
      target.source?.status === "tag-observed-artifact-link-unverified" &&
      target.source?.packageGitSha === null &&
      plan.candidate.source?.repository === target.source.repository &&
      plan.candidate.source?.status === target.source.status &&
      plan.candidate.source?.packageGitSha === target.source.packageGitSha &&
      plan.candidate.source?.tag === target.source.tag &&
      plan.candidate.source?.observedTagCommit === target.source.observedTagCommit,
    "Plan does not match the frozen 4.3.2 to 4.3.3 target record",
  );
  requireThat(
    acquisition?.package === target.package &&
      acquisition.version === target.version &&
      acquisition.integrity === target.integrity &&
      acquisition.sha256 === target.preparationAcquisitionSha256 &&
      acquisition.source?.repository === target.source.repository &&
      acquisition.source?.packageGitSha === target.source.packageGitSha &&
      assessment?.status === "ready-for-comparison" &&
      assessment.candidate?.version === target.version &&
      assessment.candidate?.integrity === target.integrity &&
      assessment.sourceIdentityStatus === target.source.status &&
      assessment.packageGitSha === target.source.packageGitSha &&
      assessment.sections?.artifacts?.tarballSha256 === target.preparationAcquisitionSha256 &&
      assessment.sections?.source?.tagCommit === target.source.observedTagCommit &&
      digest(assessment.sections?.changelog?.releaseVersions) === digest([target.version]),
    "Acquisition or assessment differs from the frozen target artifact, source, or release",
  );
  return true;
}

/** Propose a complete review-only classification from exact comparison rows. */
export function buildReviewOnlyClassification({ plan, assessment, comparison }) {
  requireThat(
    plan?.schemaVersion === 1 &&
      plan.kind === "wind-reference-plan" &&
      plan.planId === objectId(plan, "planId") &&
      plan.channel === "stable" &&
      plan.candidate?.package === "tailwindcss" &&
      plan.candidate.version === CANDIDATE_VERSION &&
      plan.previousAccepted?.version === ACCEPTED_VERSION &&
      plan.previousAccepted?.channel === "stable" &&
      plan.previousReviewedThrough?.version === ACCEPTED_VERSION,
    "Plan must be the exact stable 4.3.2 to 4.3.3 transition",
  );
  requireThat(
    assessment?.schemaVersion === 1 &&
      assessment.kind === "wind-reference-assessment" &&
      assessment.status === "ready-for-comparison" &&
      assessment.planId === plan.planId &&
      assessment.candidate?.version === CANDIDATE_VERSION &&
      assessment.candidate?.integrity === plan.candidate.integrity &&
      assessment.tagToNpmArtifactLink === "unverified" &&
      Object.values(assessment.sections ?? {}).length === 4 &&
      Object.values(assessment.sections).every((section) => section?.complete === true),
    "Assessment is incomplete, stale, or for another candidate",
  );
  requireThat(
    comparison?.schemaVersion === 1 &&
      comparison.kind === "wind-three-way-comparison" &&
      comparison.reportId === objectId(comparison, "reportId") &&
      comparison.planId === plan.planId &&
      comparison.assessmentIdentity === assessment.captureIdentity &&
      comparison.admission === "pending-independent-review-and-shipping-evidence" &&
      comparison.candidate?.reference?.version === CANDIDATE_VERSION &&
      comparison.candidate.reference.integrity === plan.candidate.integrity &&
      comparison.candidate.reference.artifactSha256 ===
        assessment.sections.artifacts.tarballSha256 &&
      comparison.accepted?.reference?.version === ACCEPTED_VERSION &&
      comparison.accepted.reference.integrity === plan.previousAccepted.integrity &&
      comparison.accepted.reference.artifactSha256 === plan.previousAccepted.artifactSha256 &&
      comparison.accepted.passing === true &&
      typeof comparison.candidate.passing === "boolean" &&
      comparison.candidate.windBuild?.gitSha === comparison.testedSourceSha &&
      comparison.inventoryMembership === "candidate-inventory-refreshed" &&
      comparison.candidateInventory &&
      comparison.testedInputs?.schemaVersion === 2,
    "Comparison is incomplete, stale, failed for the accepted baseline, or missing refreshed inventory",
  );

  const category = "unexplained-drift";
  return {
    schemaVersion: 1,
    kind: "wind-reference-classification",
    planId: plan.planId,
    comparisonReportId: comparison.reportId,
    disposition: "review-only",
    rationale:
      "Proposal preserves exact observed memberships and values. Every change remains unexplained until independent utility-level review; no candidate behavior is accepted.",
    upstreamChanges: comparison.upstreamDelta
      .filter((row) => row.changed)
      .map((row) => ({
        path: row.path,
        acceptedSha256: row.acceptedSha256,
        candidateSha256: row.candidateSha256,
        category,
        rationale: rationale(`Upstream file ${row.path}`, row.acceptedSha256, row.candidateSha256),
      })),
    browserChanges: comparison.browserDelta
      .filter((row) => row.changed)
      .map((row) => ({
        id: row.id,
        index: row.index,
        engine: row.engine,
        acceptedObservation: row.acceptedObservation,
        candidateObservation: row.candidateObservation,
        category,
        rationale: rationale(
          `Browser observation ${row.id}/${row.index}/${row.engine}`,
          row.acceptedObservation,
          row.candidateObservation,
        ),
      })),
    controlChanges: comparison.controlDelta
      .filter((row) => row.changed)
      .map((row) => ({
        id: row.id,
        engine: row.engine,
        acceptedDigest: row.acceptedDigest,
        candidateDigest: row.candidateDigest,
        category,
        rationale: rationale(
          `Control ${row.id}/${row.engine}`,
          row.acceptedDigest,
          row.candidateDigest,
        ),
      })),
    windChanges: comparison.windDelta
      .filter((row) => row.changed)
      .map((row) => ({
        path: row.path,
        acceptedSha256: row.acceptedSha256,
        candidateSha256: row.candidateSha256,
        category,
        rationale: rationale(`Wind CSS ${row.path}`, row.acceptedSha256, row.candidateSha256),
      })),
    inventoryChanges: inventoryChanges(comparison).map((row) => ({
      ...row,
      category,
      rationale: `Candidate inventory ${row.source}/${row.domain}/${row.action} ${JSON.stringify(row.value)}; utility-level disposition remains independently unreviewed.`,
    })),
  };
}

function options(argv) {
  const [command, ...args] = argv;
  if (!new Set(["propose", "rehearse"]).has(command))
    throw Error(
      "Usage: reference-rehearsal.mjs propose --evidence /outside/checkout/run | rehearse --evidence /outside/checkout/run --cache /outside/checkout/cache",
    );
  const result = { command };
  for (let index = 0; index < args.length; index += 2) {
    const name = args[index],
      value = args[index + 1];
    if (!name?.startsWith("--") || !value || value.startsWith("--"))
      throw Error(`Expected --key value, got ${name}`);
    const key = name.slice(2);
    if (!new Set(["evidence", "cache"]).has(key) || result[key] !== undefined)
      throw Error(`Unknown or duplicate option ${name}`);
    result[key] = value;
  }
  if (!result.evidence || (command === "rehearse" && !result.cache))
    throw Error(`${command} requires --evidence${command === "rehearse" ? " and --cache" : ""}`);
  if (command === "propose" && result.cache) throw Error("propose does not accept --cache");
  return result;
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
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name,
        path = resolve(directory, entry.name);
      if (entry.isDirectory()) await visit(path, relative);
      else if (entry.isFile()) rows.push([relative, sha256(await readFile(path))]);
      else throw Error(`Unsupported evidence artifact type: ${relative}`);
    }
  }
  await visit(root);
  return digest(rows);
}

function cleanCheckout() {
  return execFileSync("git", ["status", "--porcelain", "--untracked-files=all"], {
    cwd: fromRoot("."),
    encoding: "utf8",
  });
}

async function main(argv) {
  const opts = options(argv),
    evidence = await outsideCheckout(resolve(opts.evidence));
  await mkdir(evidence, { recursive: true });
  const planPath = resolve(evidence, "plan.json"),
    acquisitionPath = resolve(evidence, "acquisition.json"),
    assessmentPath = resolve(evidence, "assessment.json"),
    comparisonPath = resolve(evidence, "comparison"),
    comparisonFile = resolve(comparisonPath, "comparison.json"),
    classificationPath = resolve(evidence, "classification-proposed.json");
  await outsideCheckout(planPath);
  await outsideCheckout(acquisitionPath);
  await outsideCheckout(assessmentPath);
  await outsideCheckout(comparisonPath);
  const plan = await readJson(planPath),
    acquisition = await readJson(acquisitionPath),
    assessment = await readJson(assessmentPath),
    comparison = await readJson(comparisonFile);
  validateFrozenTarget({
    plan,
    assessment,
    acquisition,
    targetRecord: await readJson(
      fromRoot("tests/wind-compatibility/reference/manual-following-target.v1.json"),
    ),
  });
  const proposed = buildReviewOnlyClassification({ plan, assessment, comparison });

  if (opts.command === "propose") {
    const path = resolve(evidence, "classification-proposed.json");
    await outsideCheckout(path);
    await writeFile(path, JSON.stringify(proposed, null, 2) + "\n", { flag: "wx" });
    process.stdout.write(`${path}\n`);
    return;
  }

  const cache = await outsideCheckout(resolve(opts.cache));
  await outsideCheckout(classificationPath);
  await outsideCheckout(resolve(evidence, "dry-run.json"));
  await outsideCheckout(resolve(evidence, "applied.json"));
  await outsideCheckout(resolve(evidence, "state"));
  await outsideCheckout(resolve(evidence, "rehearsal-summary.json"));
  requireThat(!cleanCheckout(), "Rehearsal requires a clean source checkout");

  const acceptedPath = fromRoot(paths.accepted),
    reviewedPath = fromRoot(paths.reviewed),
    originalAccepted = await readFile(acceptedPath),
    originalReviewed = await readFile(reviewedPath),
    accepted = JSON.parse(originalAccepted.toString("utf8")),
    reviewed = JSON.parse(originalReviewed.toString("utf8")),
    classification = await readJson(classificationPath);
  requireThat(
    accepted.acceptedReference?.version === ACCEPTED_VERSION &&
      reviewed.reviewedThrough?.version === ACCEPTED_VERSION,
    "Live reference state no longer starts at admitted 4.3.2",
  );
  requireThat(
    digest(plan.previousAccepted) === digest(accepted.acceptedReference) &&
      digest(plan.previousReviewedThrough) === digest(reviewed.reviewedThrough),
    "Plan is stale against live accepted/reviewed-through state",
  );
  requireThat(
    digest(classification) === digest(proposed) &&
      classification?.disposition === "review-only" &&
      classification.rationale.includes("no candidate behavior is accepted"),
    "Disposable rehearsal requires the unchanged conservative review-only proposal",
  );
  for (const row of [
    ...(classification.upstreamChanges ?? []),
    ...(classification.browserChanges ?? []),
    ...(classification.controlChanges ?? []),
    ...(classification.windChanges ?? []),
    ...(classification.inventoryChanges ?? []),
  ])
    requireThat(
      CHANGE_CATEGORIES.has(row.category) && row.rationale?.trim(),
      "Every proposed change needs a reviewed category and rationale",
    );

  const reportInputs = [
    planPath,
    acquisitionPath,
    assessmentPath,
    classificationPath,
    comparisonFile,
  ];
  const reportInputHashes = await Promise.all(
    reportInputs.map(async (path) => [path, sha256(await readFile(path))]),
  );
  const comparisonTree = await snapshotTree(comparisonPath),
    cacheTree = await snapshotTree(cache),
    sourceSha = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: fromRoot("."),
      encoding: "utf8",
    }).trim(),
    inputs = await testedInputIdentity();
  const common = [
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
  ];
  const stateDirectory = resolve(evidence, "state");
  const transition = await runEphemeralPromotion({
    acceptedPath,
    reviewedPath,
    runDry: () => runCli(common),
    runApply: () => runCli([...common, "--apply", "yes"]),
    recover: () => runCli(["recover"]),
    validateDry: (result) =>
      requireThat(result.dryRun === true, "CLI transition dry-run mode changed"),
    validateApplied: (result) =>
      requireThat(result.dryRun === false, "CLI transition checked-apply mode changed"),
    writeDryOutput: (raw) => writeFile(resolve(evidence, "dry-run.json"), raw, { flag: "wx" }),
    writeAppliedOutput: (raw) => writeFile(resolve(evidence, "applied.json"), raw, { flag: "wx" }),
    writeStateArtifact: async ({ accepted: acceptedBytes, reviewed: reviewedBytes }) => {
      await mkdir(stateDirectory);
      await writeFile(resolve(stateDirectory, "accepted.json"), acceptedBytes, { flag: "wx" });
      await writeFile(resolve(stateDirectory, "reviewed-through.json"), reviewedBytes, {
        flag: "wx",
      });
    },
  });
  requireThat(
    digest(transition.dry.next) === digest(transition.applied.next) &&
      digest(transition.dry.testedInputs) === digest(transition.applied.testedInputs),
    "Dry-run and checked transition differ",
  );
  requireThat(
    digest(transition.dry.next.accepted) === digest(accepted) &&
      transition.dry.next.reviewed.reviewedThrough?.version === CANDIDATE_VERSION &&
      transition.dry.next.reviewed.reviewedThrough?.disposition === "review-only" &&
      transition.dry.next.reviewed.reviewedThrough?.planId === plan.planId &&
      transition.dry.next.reviewed.reviewedThrough?.comparisonReportId === comparison.reportId,
    "Checked result must retain accepted 4.3.2 and propose review-only 4.3.3",
  );
  requireThat(
    transition.state.accepted.equals(originalAccepted),
    "Proposed accepted-state artifact does not preserve the original bytes",
  );
  const afterHashes = await Promise.all(
    reportInputs.map(async (path) => [path, sha256(await readFile(path))]),
  );
  requireThat(digest(reportInputHashes) === digest(afterHashes), "Evidence input bytes changed");
  requireThat(
    comparisonTree === (await snapshotTree(comparisonPath)),
    "Comparison output tree changed",
  );
  requireThat(cacheTree === (await snapshotTree(cache)), "Reference cache tree changed");
  requireThat(
    (await readFile(acceptedPath)).equals(originalAccepted) &&
      (await readFile(reviewedPath)).equals(originalReviewed),
    "Authoritative reference state bytes were not restored exactly",
  );
  requireThat(!cleanCheckout(), "Ephemeral rehearsal left checkout changes");

  const summary = {
    schemaVersion: 1,
    kind: "wind-manual-following-rehearsal",
    authoritative: false,
    reviewStatus: "requires-independent-evidence-review-before-commit",
    currentAcceptedVersion: ACCEPTED_VERSION,
    targetVersion: CANDIDATE_VERSION,
    channel: "stable",
    planId: plan.planId,
    assessmentIdentity: assessment.captureIdentity,
    comparisonReportId: comparison.reportId,
    classificationDigest: `sha256:${digest(classification)}`,
    testedSourceSha: comparison.testedSourceSha,
    checkedTransitionSourceSha: sourceSha,
    testedInputsDigest: comparison.testedInputs.digest,
    acceptedBaselinePassing: comparison.accepted.passing,
    candidatePassing: comparison.candidate.passing,
    inventoryMembership: comparison.inventoryMembership,
    disposition: "review-only",
    proposedReviewedThroughVersion: CANDIDATE_VERSION,
    stateArtifactSha256: {
      accepted: sha256(transition.state.accepted),
      reviewedThrough: sha256(transition.state.reviewed),
    },
    proposedReviewedThrough: transition.applied.next.reviewed.reviewedThrough,
    authoritativeAcceptedStateRestored: true,
    authoritativeReviewedStateRestored: true,
    comparisonOutputPreserved: true,
    referenceCachePreserved: true,
    testedInputsPreserved: true,
    dryRunMatchesCheckedTransition: true,
    sourceIdentityStatus: assessment.sourceIdentityStatus,
    packageGitSha: assessment.packageGitSha,
    tagToNpmArtifactLink: assessment.tagToNpmArtifactLink,
    frozenTargetRecordSha256: sha256(
      await readFile(
        fromRoot("tests/wind-compatibility/reference/manual-following-target.v1.json"),
      ),
    ),
    evidenceDigests: Object.fromEntries(
      reportInputHashes.map(([path, hash]) => [path.slice(evidence.length + 1), hash]),
    ),
  };
  await writeFile(
    resolve(evidence, "rehearsal-summary.json"),
    JSON.stringify(summary, null, 2) + "\n",
    { flag: "wx" },
  );
  process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
