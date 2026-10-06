import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { readFile, writeFile, rename, unlink, mkdir, rm, stat } from "node:fs/promises";
import { resolve } from "node:path";
import { compareOutputs } from "./reference-comparison.mjs";
import { validateShippingEvidence } from "./reference-shipping.mjs";
import { assessUpstream } from "./upstream.mjs";
import { sameTestedInputs, testedInputIdentity } from "./reference-identity.mjs";
import { compareVersion, digest, fromRoot, outsideCheckout, paths, readJson, sha256 } from "./reference.mjs";

const same = (a, b, label) => {
  if (digest(a) !== digest(b)) throw Error(`${label} mismatch`);
};

export async function validateAssessment(assessment, plan, cache) {
  cache = await outsideCheckout(cache);
  if (assessment?.schemaVersion !== 1 || assessment.kind !== "wind-reference-assessment" ||
      assessment.status !== "ready-for-comparison" || assessment.planId !== plan.planId ||
      assessment.acceptedReferenceAdvanced !== false ||
      assessment.reviewedThroughAdvanced !== false ||
      assessment.tagToNpmArtifactLink !== "unverified")
    throw Error("Upstream assessment incomplete or wrong plan");
  same(assessment.candidate, plan.candidate, "assessment candidate");
  if (!Array.isArray(assessment.captures) || !assessment.captures.length ||
      assessment.captureIdentity !== `sha256:${digest(assessment.captures)}` ||
      Object.values(assessment.sections ?? {}).length !== 4 ||
      Object.values(assessment.sections).some((section) => section?.complete !== true))
    throw Error("Upstream assessment captures or sections incomplete");
  for (const capture of assessment.captures) {
    if (capture.cacheFile !== `response-${capture.sha256}` ||
        !/^https:\/\//.test(capture.url ?? "") ||
        capture.bytes <= 0 || !capture.contentType ||
        sha256(await readFile(await outsideCheckout(resolve(cache, capture.cacheFile)))) !== capture.sha256)
      throw Error(`Upstream capture missing or changed: ${capture.url}`);
  }
  let at = 0;
  const replay = await assessUpstream(plan, cache, async (url) => {
    const capture = assessment.captures[at++];
    if (!capture || capture.url !== url) throw Error(`Assessment replay URL mismatch: ${url}`);
    const bytes = await readFile(resolve(cache, capture.cacheFile));
    return { ok: true, status: 200, url, headers: { get: (name) =>
      name.toLowerCase() === "content-type" ? capture.contentType :
        name.toLowerCase() === "link" ? capture.link : null },
      arrayBuffer: async () => bytes };
  });
  if (at !== assessment.captures.length || replay.status !== "ready-for-comparison" ||
      digest(replay) !== digest(assessment))
    throw Error("Assessment does not replay from retained upstream captures");
  if (assessment.sections.artifacts.integrity !== plan.candidate.integrity ||
      assessment.sections.artifacts.packageVersion !== plan.candidate.version ||
      assessment.sections.artifacts.tarballSha256 !==
        (plan.candidate.artifactSha256 ?? assessment.sections.artifacts.tarballSha256))
    throw Error("Assessed tarball differs from candidate");
  return assessment.sections.artifacts.tarballSha256;
}

function shaReport(report) {
  const { reportId: _id, ...body } = report;
  return `sha256:${digest(body)}`;
}

export async function validateComparison({ comparison, plan, assessment, output, candidate, accepted,
  input, cache, strictArtifacts = false }) {
  if (comparison?.reportId !== shaReport(comparison) ||
      comparison.kind !== "wind-three-way-comparison" ||
      comparison.planId !== plan.planId ||
      comparison.assessmentIdentity !== assessment.captureIdentity ||
      comparison.admission !== "pending-independent-review-and-shipping-evidence")
    throw Error("Comparison report invalid or stale");
  const replay = await compareOutputs({ plan, assessment, output, candidate, accepted, input,
    cache, strictArtifacts, artifactOrigin: comparison.artifactOrigin });
  same(comparison, replay, "executed three-way comparison");
  if (!sameTestedInputs(comparison.testedInputs, await testedInputIdentity()))
    throw Error("Tested input closure changed after comparison");
  const source = execFileSync("git", ["rev-parse", "HEAD"], { cwd: fromRoot("."), encoding: "utf8" }).trim();
  if (comparison.testedSourceSha !== source && !sameTestedInputs(
    comparison.testedInputs, await testedInputIdentity()))
    throw Error("Source changed after tested commit");
  return comparison;
}

export function validateClassification(record, comparison, plan) {
  if (record?.schemaVersion !== 1 || record.kind !== "wind-reference-classification" ||
      record.planId !== plan.planId || record.comparisonReportId !== comparison.reportId ||
      !["accept", "review-only"].includes(record.disposition) ||
      !Array.isArray(record.upstreamChanges) || !Array.isArray(record.browserChanges) ||
      !Array.isArray(record.inventoryChanges))
    throw Error("Missing actual reviewed classification record");
  const expected = comparison.upstreamDelta.filter((row) => row.changed);
  same(record.upstreamChanges.map((row) => row.path), expected.map((row) => row.path),
    "upstream change membership");
  const categories = new Set(["upstream-semantic", "upstream-default-theme",
    "upstream-packaging", "intentional-exclusion", "local-implementation-gap", "unexplained-drift"]);
  for (let index = 0; index < expected.length; index++) {
    const row = record.upstreamChanges[index];
    if (row.acceptedSha256 !== expected[index].acceptedSha256 ||
        row.candidateSha256 !== expected[index].candidateSha256 ||
        !categories.has(row.category) || !row.rationale?.trim())
      throw Error(`Unclassified upstream change ${expected[index].path}`);
  }
  const expectedBrowser = comparison.browserDelta.filter((row) => row.changed);
  same(record.browserChanges.map((row) => [row.id, row.index, row.engine]),
    expectedBrowser.map((row) => [row.id, row.index, row.engine]),
    "browser change membership");
  for (let index = 0; index < expectedBrowser.length; index++) {
    const row = record.browserChanges[index], observed = expectedBrowser[index];
    if (!categories.has(row.category) || !row.rationale?.trim() ||
        digest(row.acceptedObservation) !== digest(observed.acceptedObservation) ||
        digest(row.candidateObservation) !== digest(observed.candidateObservation))
      throw Error(`Unclassified browser change ${observed.id}/${observed.index}`);
  }
  if (record.disposition === "accept" &&
      [...record.upstreamChanges, ...record.browserChanges]
        .some((row) => row.category === "unexplained-drift"))
    throw Error("Unexplained drift cannot be accepted");
  if (plan.previousAccepted && comparison.inventoryMembership !== "candidate-inventory-refreshed")
    throw Error("Candidate upstream inventory membership was not refreshed");
  const inventoryChanges = [];
  for (const [source, changes] of Object.entries({
    runtime: comparison.candidateInventory?.runtimeChanges ?? {},
    source: comparison.candidateInventory?.sourceChanges ?? {},
  }))
    for (const [domain, actions] of Object.entries(changes))
      for (const action of ["added", "removed"])
        for (const value of actions[action] ?? [])
          inventoryChanges.push({ source, domain, action, value });
  for (const [domain, value] of Object.entries(
    comparison.candidateInventory?.sourceMetadataChanges ?? {}))
    inventoryChanges.push({ source: "source-metadata", domain,
      action: "changed", value });
  same(record.inventoryChanges.map(({ source, domain, action, value }) =>
    ({ source, domain, action, value })), inventoryChanges, "candidate inventory membership");
  for (const row of record.inventoryChanges)
    if (!categories.has(row.category) || !row.rationale?.trim())
      throw Error("Unclassified candidate inventory change");
  if (record.disposition === "accept" &&
      record.inventoryChanges.some((row) => row.category === "unexplained-drift"))
    throw Error("Unexplained inventory drift cannot be accepted");
  return true;
}

export async function transition({ plan, assessment, comparison, classification, shipping, shippingOutput, state, finalSha }) {
  validateClassification(classification, comparison, plan);
  const reviewed = {
    schemaVersion: 1,
    reviewedThrough: {
      package: "tailwindcss", version: plan.candidate.version, channel: plan.channel,
      integrity: plan.candidate.integrity, disposition: classification.disposition,
      planId: plan.planId, assessmentId: assessment.captureIdentity,
      comparisonReportId: comparison.reportId, classificationDigest: `sha256:${digest(classification)}`,
      testedSourceSha: comparison.testedSourceSha, finalVerificationSha: finalSha,
      testedInputsDigest: comparison.testedInputs.digest,
    },
  };
  if (classification.disposition !== "accept")
    return { accepted: state.accepted, reviewed };
  if (comparison.candidate.passing !== true ||
      (comparison.accepted && comparison.accepted.passing !== true))
    throw Error("Acceptance requires every mandatory comparison to pass");
  if (comparison.browserDelta.some((row) => row.localChanged === true))
    throw Error("Local Wind/browser environment changed during three-way comparison");
  const shippingIdentity = await validateShippingEvidence(shipping, comparison, shippingOutput, plan.toolchain);
  const candidate = comparison.candidate.reference;
  if (state.accepted.acceptedReference &&
      compareVersion(plan.candidate.version, state.accepted.acceptedReference.version) <= 0)
    throw Error("Accepted reference cannot move backwards");
  return {
    accepted: {
      schemaVersion: 1,
      acceptedReference: {
        package: "tailwindcss", version: plan.candidate.version, channel: plan.channel,
        integrity: candidate.integrity, artifactSha256: candidate.artifactSha256,
        lockfileSha256: plan.inputs.lock, toolchain: plan.toolchain,
        source: plan.candidate.source, profileId: comparison.profile.id,
        profileVersion: comparison.profile.version, profileRevision: comparison.profile.revision,
        profileDigest: comparison.profile.digest, evidenceReportId: comparison.reportId,
        evidenceReportDigest: `sha256:${digest(comparison)}`,
        testedSourceSha: comparison.testedSourceSha, finalVerificationSha: finalSha,
        testedInputsDigest: comparison.testedInputs.digest,
        shippingEvidenceDigest: shippingIdentity.evidenceDigest,
        shippingManifestDigest: shippingIdentity.manifestDigest,
      },
    },
    reviewed,
  };
}

export async function writeTransitionAtomically(next, expected, overrides = {}) {
  const acceptedPath = overrides.acceptedPath ?? fromRoot(paths.accepted);
  const reviewedPath = overrides.reviewedPath ?? fromRoot(paths.reviewed);
  const journal = overrides.journalPath ?? journalPath;
  const lock = overrides.lockPath ?? lockPath;
  const renameFile = overrides.renameFile ?? rename;
  return withTransitionLock(async () => {
  // The journal makes an interrupted two-file replacement recoverable. Every
  // repository-owned CLI read and writer enters this lock/recovery boundary.
  const oldAccepted = await readFile(acceptedPath), oldReviewed = await readFile(reviewedPath);
  same(JSON.parse(oldAccepted), expected.accepted, "accepted state");
  same(JSON.parse(oldReviewed), expected.reviewed, "reviewed state");
  const suffix = randomUUID();
  const stagedA = `${acceptedPath}.${suffix}.tmp`, stagedR = `${reviewedPath}.${suffix}.tmp`;
  const stagedJ = `${journal}.${suffix}.tmp`;
  try {
    await writeFile(stagedA, JSON.stringify(next.accepted, null, 2) + "\n", { flag: "wx" });
    await writeFile(stagedR, JSON.stringify(next.reviewed, null, 2) + "\n", { flag: "wx" });
    await writeFile(stagedJ, JSON.stringify({ schemaVersion: 1,
      oldAccepted: oldAccepted.toString("utf8"), oldReviewed: oldReviewed.toString("utf8") }),
    { flag: "wx" });
    await rename(stagedJ, journal);
    await renameFile(stagedA, acceptedPath);
    await renameFile(stagedR, reviewedPath);
    await unlink(journal);
  } catch (error) {
    await restoreJournal({ acceptedPath, reviewedPath, journalPath: journal });
    throw error;
  } finally {
    await unlink(stagedA).catch(() => {});
    await unlink(stagedR).catch(() => {});
    await unlink(stagedJ).catch(() => {});
  }
  }, { lockPath: lock, journalPath: journal, acceptedPath, reviewedPath });
}

const lockPath = fromRoot("tests/wind-compatibility/reference/.transition-lock");
const journalPath = fromRoot("tests/wind-compatibility/reference/.transition-journal.json");

export async function assertStableTransition() {
  for (const path of [lockPath, journalPath]) {
    try { await stat(path); throw Error("Reference transition active or recovery required"); }
    catch (error) { if (error.code !== "ENOENT") throw error; }
  }
}

export async function recoverTransition() {
  return withTransitionLock(async () => ({ recovered: true }));
}

async function restoreJournal({ acceptedPath = fromRoot(paths.accepted),
  reviewedPath = fromRoot(paths.reviewed), journalPath: journalFile = journalPath } = {}) {
  let journal;
  try { journal = await readJson(journalFile); }
  catch (error) { if (error.code === "ENOENT") return; throw error; }
  if (journal.schemaVersion !== 1 ||
      typeof journal.oldAccepted !== "string" || typeof journal.oldReviewed !== "string")
    throw Error("Invalid transition journal; manual recovery required");
  await writeFile(acceptedPath, journal.oldAccepted);
  await writeFile(reviewedPath, journal.oldReviewed);
  await unlink(journalFile);
}

export async function withTransitionLock(fn, { lockPath: lock = lockPath,
  journalPath: journal = journalPath, acceptedPath = fromRoot(paths.accepted),
  reviewedPath = fromRoot(paths.reviewed) } = {}) {
  try { await mkdir(lock); }
  catch (error) {
    if (error.code !== "EEXIST") throw error;
    let owner;
    try { owner = Number(await readFile(resolve(lock, "pid"), "utf8")); }
    catch { throw Error("Transition lock exists without owner; manual recovery required"); }
    try { process.kill(owner, 0); throw Error("Reference transition already active"); }
    catch (probe) {
      if (probe.code !== "ESRCH") throw probe;
      await rm(lock, { recursive: true });
      await mkdir(lock);
    }
  }
  try {
    await writeFile(resolve(lock, "pid"), String(process.pid));
    await restoreJournal({ acceptedPath, reviewedPath, journalPath: journal });
    return await fn();
  } finally { await rm(lock, { recursive: true, force: true }); }
}
