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

const REVIEWED_PILOT_IDS = Object.freeze([
  "block",
  "hidden",
  "inline-flex",
  "mx-auto",
  "grid-cols-2",
  "p-0-empty",
  "p-0-mapped",
  "named-spacing",
  "named-color",
  "unconfigured-p-4",
  "undeclared-palette",
  "configured-p-4",
  "contents-gap",
  "hover-block",
  "breakpoint-block",
]);

export function validatePilot(profile, manifest, observations) {
  if (
    profile.profileId !== "wind-preset-free" ||
    profile.profileVersion !== 1 ||
    profile.profileRevision !== 5 ||
    profile.sourceBaseline?.languageSpecVersion !== 1 ||
    profile.sourceBaseline?.languageSpecRevision !== 14
  )
    throw Error("Unexpected compatibility profile");
  if (
    !Array.isArray(profile.requiredCases) ||
    !Array.isArray(manifest.caseIds) ||
    !observations ||
    typeof observations !== "object" ||
    Array.isArray(observations)
  )
    throw Error("Invalid pilot membership input");
  const profileIds = profile.requiredCases.map((row) => row.id);
  if (
    digest(profileIds) !== digest(REVIEWED_PILOT_IDS) ||
    digest(manifest.caseIds) !== digest(REVIEWED_PILOT_IDS) ||
    digest(Object.keys(observations)) !== digest(REVIEWED_PILOT_IDS)
  )
    throw Error("Pilot membership differs from reviewed 15-case set");
  const required = new Map(profile.requiredCases.map((row) => [row.id, row]));
  const differences = new Map(profile.reviewedDifferences.map((row) => [row.id, row]));
  const ids = manifest.caseIds;
  if (!Array.isArray(ids) || new Set(ids).size !== ids.length || ids.length === 0)
    throw Error("Invalid pilot manifest");
  const contents = required.get("contents-gap");
  if (
    contents?.candidate !== "contents" ||
    contents.disposition !== "equivalent-shared" ||
    contents.implementation !== "implemented" ||
    contents.windTokenFreeGuarantee !== true ||
    digest(contents.reviewedDifferenceIds) !== digest(["wind-layer-order-prelude"])
  )
    throw Error("contents-gap must retain its implemented shared classification");
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
  const mx = new Set(observations["mx-auto"]?.probes.map((probe) => probe.name));
  for (const mode of ["horizontal-tb", "vertical-rl"])
    for (const direction of ["ltr", "rtl"])
      for (const measure of [
        "margin-left",
        "margin-right",
        "margin-top",
        "margin-bottom",
        "geometry-left",
        "geometry-top",
      ])
        if (!mx.has(`${mode}-${direction}-${measure}`))
          throw Error(`Missing axis probe ${mode}/${direction}/${measure}`);
  const webkit = profile.browserPolicy.requiredMatrix.find((row) => row.browser === "webkit");
  if (
    webkit?.browserVersion !== "26.5" ||
    webkit.revision !== "2311" ||
    webkit.hostPlatform !== "ubuntu24.04-x64"
  )
    throw Error("Pinned WebKit resolved margin browser changed without policy review");
  const overrideNames = new Set(["vertical-rl-ltr-margin-bottom", "vertical-rl-rtl-margin-top"]);
  const nativeReportSha256 = "afb7f2b557900acdbe061f0fe48ec4b2ae5375cd79f9394b15138072319acbe4";
  for (const probe of observations["mx-auto"].probes) {
    if (overrideNames.has(probe.name)) {
      const expected = {
        browser: "webkit",
        browserVersion: webkit?.browserVersion,
        revision: webkit?.revision,
        hostPlatform: webkit?.hostPlatform,
        value: "200px",
        nativeRun: 37405347552,
        nativeReportSha256,
      };
      if (
        probe.wind !== "0px" ||
        probe.reference !== "100px" ||
        !probe.windBrowserOverride ||
        digest(probe.windBrowserOverride) !== digest(expected)
      )
        throw Error(`Unreviewed WebKit resolved margin expectation: ${probe.name}`);
    } else if (probe.windBrowserOverride !== undefined) {
      throw Error(`Unreviewed browser override: ${probe.name}`);
    }
  }
  for (const mode of ["horizontal-tb", "vertical-rl"])
    for (const direction of ["ltr", "rtl"])
      for (const axis of ["left", "top"]) {
        const probe = observations["mx-auto"].probes.find(
          (row) => row.name === `${mode}-${direction}-geometry-${axis}`,
        );
        const expected =
          mode === "horizontal-tb"
            ? [axis === "left" ? "100" : "0", axis === "left" ? "100" : "0"]
            : axis === "left"
              ? ["200", "200"]
              : [direction === "ltr" ? "0" : "200", "100"];
        if (probe.wind !== expected[0] || probe.reference !== expected[1])
          throw Error(`Unreviewed mx-auto geometry: ${probe.name}`);
      }
  if (
    !observations["grid-cols-2"]?.probes.some(
      (probe) => probe.selector === "#child-b" && probe.property.startsWith("geometry:"),
    ) ||
    !observations["inline-flex"]?.probes.some(
      (probe) => probe.selector === "#child-b" && probe.property.startsWith("geometry:"),
    )
  )
    throw Error("Grid and inline-flex child geometry is required");
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
  return !["unconfigured-p-4", "undeclared-palette"].includes(row.id);
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

export function compareExtractionSets(actual, reviewed) {
  const unique = (values) => Array.isArray(values) && new Set(values).size === values.length;
  if (
    ![
      actual.windCandidates,
      actual.referenceCandidates,
      actual.windRules,
      actual.referenceRules,
    ].every(unique)
  )
    return false;
  return (
    digest(actual.windCandidates) === digest(reviewed.expectedWindCandidates) &&
    digest(actual.referenceCandidates) === digest(reviewed.expectedReferenceCandidates) &&
    digest([...actual.windRules].sort()) === digest([...reviewed.expectedWindRules].sort()) &&
    digest([...actual.referenceRules].sort()) ===
      digest([...reviewed.expectedReferenceRules].sort())
  );
}
