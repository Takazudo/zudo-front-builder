import { digest } from "./reference.mjs";
import { validatePilot } from "./differential-core.mjs";

const fixed = Object.freeze({
  pilot: 15,
  supplemental: 4,
  nativeGuarantees: 18,
  upstreamCases: 19,
  upstreamProbes: 55,
  seededSpecimens: 10,
  seededArbitraryValues: 6,
  seededSourceStrings: 6,
  mutationControls: 5,
});
const engines = ["chromium", "firefox", "webkit"];
const targetedEngines = new Set([
  "hover-flex",
  "focus-flex",
  "group-focus-flex",
  "peer-focus-flex",
  "padding-axis",
  "margin-axis",
  "dark-flex",
  "pseudo-before",
  "translate-composed",
  "space-hidden-children",
]);
const reviewedTargetedExecutionKeys = Object.freeze({
  "p-0-mapped": ["pilot/p-0-mapped"],
  "named-spacing": ["pilot/named-spacing"],
  "named-color": ["pilot/named-color"],
  "hover-block": ["pilot/hover-block"],
  "breakpoint-block": ["pilot/breakpoint-block"],
  "grid-cols-2": ["pilot/grid-cols-2"],
  "inline-flex": ["pilot/inline-flex"],
  "mx-auto": ["pilot/mx-auto"],
  "nested-token-scope": ["control/nested-token-scope"],
  "specificity-and-authored-cascade": ["control/specificity-and-authored-cascade"],
  "native-reset-controls": ["control/native-reset-controls"],
  "wrong-value-or-missing-rule-detection": [
    "control/wrong-value-detection",
    "control/missing-rule-detection",
  ],
});

export function validateTargetedExecutionKeys(manifest, profile) {
  const mapping = manifest.targetedExecutionKeys;
  if (
    !mapping ||
    digest(Object.keys(mapping)) !== digest(profile.browserPolicy.targetedObligations) ||
    digest(mapping) !== digest(reviewedTargetedExecutionKeys)
  )
    throw Error("Targeted browser execution mapping changed without review");
  for (const [obligation, keys] of Object.entries(mapping)) {
    if (!Array.isArray(keys) || !keys.length || new Set(keys).size !== keys.length)
      throw Error(`Duplicate or missing targeted execution key: ${obligation}`);
    for (const key of keys) {
      const [lane, id, extra] = key.split("/");
      if (
        extra ||
        !(
          (lane === "pilot" && manifest.pilotCaseIds.includes(id)) ||
          (lane === "control" && profile.requiredControls.includes(id))
        )
      )
        throw Error(`Targeted execution key is not a current pilot/control: ${key}`);
    }
  }
  return mapping;
}

const compositionDigest = "f1a12520b9f4216ff71013771764b7dcbe87dde88b3e0f9c43b892486f62b153";
const probePolicyDigest = "9bc08e42b312f476a59a39355fa260fb07d288302c9a621ffc5f19cbffa78368";
const reviewedManifestDigest = "8694d907e81664ddc4f7d61f19650f703df1a840b52611ab5f9a892765cb8772";
const mutationIds = [
  "missing-stylesheet",
  "missing-rule",
  "wrong-declaration",
  "wrong-selector",
  "wrong-media",
];

export function validateCorpus(
  manifest,
  profile,
  empty,
  pilot,
  upstream,
  pilotObservations,
  probesById,
) {
  if (manifest.schemaVersion !== 1 || manifest.issue !== 3831) throw Error("Unknown corpus policy");
  if (
    manifest.profile !== `wind-preset-free@${profile.profileVersion}.${profile.profileRevision}` ||
    profile.profileRevision !== 3
  )
    throw Error("Corpus is not bound to the reviewed profile revision");
  validatePilot(profile, pilot, pilotObservations);
  validateTargetedExecutionKeys(manifest, profile);
  if (digest(manifest.counts) !== digest(fixed))
    throw Error("Corpus counts changed without policy review");
  const lanes = [
    ["pilot", manifest.pilotCaseIds, profile.requiredCases.map((row) => row.id)],
    ["supplemental", manifest.supplementalCaseIds, empty.supplementalCaseIds],
    ["native", manifest.nativeGuaranteeIds, empty.nativeGuarantees.map((row) => row.id)],
    ["upstream", manifest.upstreamCases.map((row) => row.id), upstream.caseIds],
  ];
  for (const [lane, actual, expected] of lanes) {
    if (
      !Array.isArray(actual) ||
      !actual.length ||
      new Set(actual).size !== actual.length ||
      digest(actual) !== digest(expected)
    )
      throw Error(`${lane} membership changed or duplicate ID`);
  }
  if (digest(manifest.pilotCaseIds) !== digest(pilot.caseIds))
    throw Error("Pilot manifest mismatch");
  if (
    digest(manifest.pilotPolicies) !==
    digest(
      profile.requiredCases.map(({ id, disposition, reviewedDifferenceIds }) => ({
        id,
        disposition,
        reviewedDifferenceIds,
      })),
    )
  )
    throw Error("Pilot disposition/difference policy changed");
  const seen = new Set();
  let probeCount = 0;
  for (const row of manifest.upstreamCases) {
    if (seen.has(row.id)) throw Error(`Duplicate upstream ID ${row.id}`);
    seen.add(row.id);
    if (
      !row.upstreamPath?.startsWith("packages/tailwindcss/src/") ||
      !row.upstreamTest ||
      !row.originalInput ||
      !row.adaptation
    )
      throw Error(`Incomplete upstream provenance ${row.id}`);
    if (
      !Array.isArray(row.candidates) ||
      !row.candidates.length ||
      (row.id !== "duplicate-reordered" &&
        new Set(row.candidates).size !== row.candidates.length) ||
      (row.id === "duplicate-reordered" &&
        digest(row.candidates) !== digest(["flex", "flex-row", "flex"]))
    )
      throw Error(`Empty or duplicate candidates ${row.id}`);
    if (
      !Array.isArray(row.engines) ||
      !row.engines.includes("chromium") ||
      new Set(row.engines).size !== row.engines.length ||
      row.engines.some((x) => !engines.includes(x))
    )
      throw Error(`Invalid engine coverage ${row.id}`);
    if (digest(row.engines) !== digest(targetedEngines.has(row.id) ? engines : ["chromium"]))
      throw Error(`Engine obligation downgraded ${row.id}`);
    const probes = probesById[row.id];
    if (
      !Array.isArray(probes) ||
      !probes.length ||
      new Set(probes.map((x) => x.name)).size !== probes.length
    )
      throw Error(`Missing or duplicate probes ${row.id}`);
    for (const probe of probes) {
      if (
        !probe.name ||
        !probe.property ||
        probe.wind === undefined ||
        probe.reference === undefined ||
        probe.wind === null ||
        probe.reference === null
      )
        throw Error(`Incomplete expected state ${row.id}/${probe.name}`);
    }
    probeCount += probes.length;
  }
  if (probeCount !== fixed.upstreamProbes || Object.keys(probesById).length !== fixed.upstreamCases)
    throw Error("Upstream probe accounting drift");
  if (digest(probesById) !== probePolicyDigest)
    throw Error("Upstream observation policy changed without review");
  const reviewed = {
    "exact-tree-contract": manifest.upstreamCases.map((row) => row.id),
    "physical-logical-axis-composition": ["padding-axis", "margin-axis"],
    "pseudo-default-content": ["pseudo-before"],
    "space-child-selection-axis": ["space-hidden-children"],
  };
  if (
    digest(
      manifest.reviewedDifferences.map(({ id, caseIds, reviewedCount }) => ({
        id,
        caseIds,
        reviewedCount,
      })),
    ) !==
    digest(
      Object.entries(reviewed).map(([id, caseIds]) => ({
        id,
        caseIds,
        reviewedCount: caseIds.length,
      })),
    )
  )
    throw Error("Reviewed difference membership changed");
  for (const row of manifest.upstreamCases) {
    const expected = Object.entries(reviewed)
      .filter(([, ids]) => ids.includes(row.id))
      .map(([id]) => id);
    if (digest(row.reviewedDifferenceIds) !== digest(expected))
      throw Error(`Unreviewed difference classification ${row.id}`);
  }
  const requiredMechanisms = [
    "shorthand-axis-side",
    "duplicate-reordered-input",
    "responsive-threshold",
    "hover-capability",
    "keyboard-focus",
    "group-peer-relationships",
    "dark-self-ancestor",
    "pseudo-content",
    "translation-composition",
    "hidden-versus-display-none",
    "authored-reset-layer",
    "writing-mode-direction",
    "physical-logical-axes",
    "wrong-output-detection",
  ];
  if (
    digest(Object.keys(manifest.compositionObligations ?? {}).sort()) !==
    digest(requiredMechanisms.sort())
  )
    throw Error("Required composition mechanism removed");
  if (digest(manifest.compositionObligations) !== compositionDigest)
    throw Error("Composition condition membership changed without policy review");
  for (const [mechanism, refs] of Object.entries(manifest.compositionObligations)) {
    if (!Array.isArray(refs) || !refs.length || new Set(refs).size !== refs.length)
      throw Error(`Empty or duplicate composition references ${mechanism}`);
    for (const ref of refs) {
      const [lane, id, probe] = ref.split("/");
      const found =
        lane === "control"
          ? profile.requiredControls.includes(id) && !probe
          : lane === "pilot"
            ? manifest.pilotCaseIds.includes(id) &&
              pilotObservations[id]?.probes.some((row) => row.name === probe)
            : lane === "upstream"
              ? manifest.upstreamCases.some((row) => row.id === id) &&
                probesById[id]?.some((row) => row.name === probe)
              : false;
      if (!found) throw Error(`Composition reference has no executable probe: ${mechanism}/${ref}`);
    }
  }
  if (digest(manifest) !== reviewedManifestDigest)
    throw Error("Checked corpus policy changed without review");
  return expectedObligations(manifest);
}

export function expectedObligations(manifest) {
  const ids = [
    ...engines.flatMap((engine) => manifest.pilotCaseIds.map((id) => `pilot/${id}/${engine}`)),
    ...manifest.supplementalCaseIds.map((id) => `supplemental/${id}/chromium`),
    ...manifest.nativeGuaranteeIds.map((id) => `native/${id}/chromium`),
    ...manifest.upstreamCases.flatMap((row) =>
      row.engines.map((engine) => `upstream/${row.id}/${engine}`),
    ),
  ];
  if (new Set(ids).size !== ids.length) throw Error("Duplicate corpus obligation");
  return ids;
}

export function expectedOutcomes(manifest, profile) {
  const pilot = {
    "equivalent-shared": "matched",
    "equivalent-mapped": "mapped-match",
    "intentional-difference": "reviewed-difference",
    "implementation-gap": "expected-unsupported",
  };
  return Object.fromEntries([
    ...engines.flatMap((engine) =>
      profile.requiredCases.map((row) => [`pilot/${row.id}/${engine}`, pilot[row.disposition]]),
    ),
    ...manifest.supplementalCaseIds.map((id) => [`supplemental/${id}/chromium`, "matched"]),
    ...manifest.nativeGuaranteeIds.map((id) => [`native/${id}/chromium`, "matched"]),
    ...manifest.upstreamCases.flatMap((row) =>
      row.engines.map((engine) => [
        `upstream/${row.id}/${engine}`,
        row.reviewedDifferenceIds.length > 1 ? "reviewed-difference" : "matched",
      ]),
    ),
  ]);
}

export function completeCorpus(expected, executed, allowedOutcomes = {}) {
  const keys = Object.keys(executed);
  if (new Set(expected).size !== expected.length || keys.some((key) => !expected.includes(key)))
    throw Error("Unreviewed or duplicate corpus execution");
  const missing = expected.filter((id) => !Object.hasOwn(executed, id));
  const failures = keys.filter(
    (id) => executed[id]?.outcome !== (allowedOutcomes[id] ?? "matched"),
  );
  return {
    complete: missing.length === 0 && failures.length === 0,
    expectedCount: expected.length,
    executedCount: keys.length,
    missing,
    failures,
  };
}

export function checkMutations(valid, mutated) {
  if (
    digest(Object.keys(valid).sort()) !== digest(["display-valid", "hover-valid"].sort()) ||
    digest(Object.keys(mutated).sort()) !== digest([...mutationIds].sort())
  )
    throw Error("Mutation control was skipped, added or renamed");
  if (!Object.values(valid).every((result) => result === true))
    throw Error("Valid control did not pass");
  if (!Object.values(mutated).every((result) => result === false))
    throw Error("Mutation survived corpus observation");
  return {
    outcome: "matched",
    validControls: valid,
    mutations: mutated,
    killedMutations: mutationIds.length,
  };
}

export function finiteInventoryAccounting(inventory, catalog, manifest, empty, profile) {
  const candidates = new Set([
    ...profile.requiredCases.map((row) => row.candidate.split(":").at(-1)),
    ...empty.nativeGuarantees.map((row) => row.candidate),
    ...manifest.upstreamCases.flatMap((row) =>
      row.candidates.map((candidate) => candidate.split(":").at(-1)),
    ),
  ]);
  const rows = inventory.rows.filter(
    (row) => row.upstream?.kind === "utility-static" && row.windMapping?.kind === "exact-catalog",
  );
  const covered = rows.filter((row) => candidates.has(row.upstream.name));
  const omitted = rows.filter((row) => !candidates.has(row.upstream.name));
  if (!rows.length || covered.length + omitted.length !== rows.length)
    throw Error("Finite inventory accounting failed");
  const keywordDomain = catalog.entries.flatMap((entry) =>
    entry.acceptedValues
      .filter((value) => value.kind === "keyword" || value.kind === "fallbackKeyword")
      .map((value) => ({ catalogId: entry.id, candidate: `${entry.root}-${value.suffix}` })),
  );
  const keywordCovered = keywordDomain.filter((row) => candidates.has(row.candidate));
  const keywordOmitted = keywordDomain.filter((row) => !candidates.has(row.candidate));
  return {
    domain: "source-inspected exact-catalog upstream static registrations",
    exhaustive: false,
    domainCount: rows.length,
    coveredCount: covered.length,
    omittedCount: omitted.length,
    coveredRowIds: covered.map((row) => row.id),
    omittedRowIds: omitted.map((row) => row.id),
    keywordDomain: {
      scope:
        "finite positive keyword/fallbackKeyword members in Wind catalog; source-inspected, not all admitted shared behavior",
      domainCount: keywordDomain.length,
      coveredCount: keywordCovered.length,
      omittedCount: keywordOmitted.length,
      covered: keywordCovered,
      omitted: keywordOmitted,
    },
  };
}
