import { digest, sha256 } from "./reference.mjs";

export const outcomes = Object.freeze({
  shared: "matched",
  mapped: "mapped-match",
  difference: "reviewed-difference",
  unsupported: "expected-unsupported",
  mismatch: "unexpected-mismatch",
  infrastructure: "infrastructure-failure",
  missing: "not-executed",
});

export function validatePilot(profile, manifest, observations) {
  if (profile.profileId !== "wind-preset-free" || profile.profileVersion !== 1)
    throw Error("Unexpected compatibility profile");
  const required = new Map(profile.requiredCases.map((row) => [row.id, row]));
  const differences = new Map(profile.reviewedDifferences.map((row) => [row.id, row]));
  const ids = manifest.caseIds;
  if (!Array.isArray(ids) || new Set(ids).size !== ids.length || ids.length === 0)
    throw Error("Invalid pilot manifest");
  for (const id of ids) {
    if (!required.has(id)) throw Error(`Unreviewed pilot case ${id}`);
    for (const differenceId of required.get(id).reviewedDifferenceIds) {
      if (!differences.get(differenceId)?.caseIds.includes(id))
        throw Error(`Unreviewed difference membership ${id}/${differenceId}`);
    }
    const spec = observations[id];
    if (!spec || spec.caseId !== id || !Array.isArray(spec.probes) || !spec.probes.length)
      throw Error(`Missing observations for ${id}`);
    for (const probe of spec.probes) {
      if (!probe.name || !probe.property || !probe.wind || !probe.reference)
        throw Error(`Incomplete observation ${id}/${probe.name}`);
    }
  }
  for (const id of Object.keys(observations))
    if (!required.has(id) || !ids.includes(id)) throw Error(`Unlisted observation ${id}`);
  return required;
}

export function preludeCheck(css, expectedNonempty, statement) {
  const matches = [
    ...css.matchAll(
      /@layer\s+zw-reset\s*,\s*zw-tokens\s*,\s*zfb-hi\s*,\s*base\s*,\s*components\s*;/g,
    ),
  ];
  if (matches.length !== Number(expectedNonempty)) return false;
  if (expectedNonempty && (matches[0].index !== 0 || !css.startsWith(statement))) return false;
  return !css.includes("@layer zw-reset", expectedNonempty ? statement.length : 0);
}

export function requiredNonempty(row) {
  return !["unconfigured-p-4", "undeclared-palette", "contents-gap"].includes(row.id);
}

export function classify(row, checks) {
  if (checks.infrastructure) return outcomes.infrastructure;
  if (checks.missing) return outcomes.missing;
  if (!checks.identity || !checks.structure || !checks.observations || !checks.diagnostics)
    return outcomes.mismatch;
  if (row.disposition === "equivalent-shared") return outcomes.shared;
  if (row.disposition === "equivalent-mapped") return outcomes.mapped;
  if (row.disposition === "intentional-difference") return outcomes.difference;
  if (row.disposition === "implementation-gap") return outcomes.unsupported;
  return outcomes.mismatch;
}

export function completeReport(profile, manifest, rows, controls, identity) {
  const ids = new Set(manifest.caseIds);
  const results = profile.requiredCases.map(
    (row) =>
      rows[row.id] ?? {
        caseId: row.id,
        outcome: outcomes.missing,
        reason: ids.has(row.id) ? "execution absent" : "outside pilot manifest",
      },
  );
  const controlResults = profile.requiredControls.map(
    (id) =>
      controls[id] ?? {
        controlId: id,
        outcome: outcomes.missing,
        reason: "control not executed",
      },
  );
  const requiredResults = results.filter((row) => ids.has(row.caseId));
  const failed = [...requiredResults, ...controlResults].some((row) =>
    [outcomes.mismatch, outcomes.infrastructure, outcomes.missing].includes(row.outcome),
  );
  const body = {
    schemaVersion: 1,
    kind: "wind-differential-pilot",
    identity,
    pilotCaseIds: [...ids],
    cases: results,
    controls: controlResults,
    admission: "pilot-only",
    complete: !failed && ids.size === profile.requiredCases.length,
  };
  return {
    ...body,
    reportId: `sha256:${digest(body)}`,
    exitCode: failed || ids.size !== profile.requiredCases.length ? 1 : 0,
  };
}

export function artifactIdentity(css, report) {
  return { cssSha256: sha256(css), reportSha256: sha256(JSON.stringify(report)) };
}
