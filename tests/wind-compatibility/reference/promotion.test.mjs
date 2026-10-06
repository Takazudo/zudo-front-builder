import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm, rename, symlink, cp, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  testedPaths,
  sameTestedInputs,
} from "../../../scripts/wind-compatibility/reference-identity.mjs";
import {
  assertStableTransition,
  monotonicReviewed,
  upstreamReviewTransition,
  validateClassification,
  validateUpstreamReview,
  transition,
  writeTransitionAtomically,
  withTransitionLock,
} from "../../../scripts/wind-compatibility/reference-promotion.mjs";
import { digest } from "../../../scripts/wind-compatibility/reference.mjs";
import { validateShippingManifest } from "../../../scripts/wind-compatibility/reference-shipping.mjs";
import {
  assertSameBrowserEnvironment,
  observationRows,
  readRecordedArtifact,
  requirePassingCurrent,
  validateCorpusProbeOutcomes,
  validateNativeRawRow,
  validateSupplementalRawRow,
  validateSeededArtifactHashes,
  validateSeededMembership,
  validateRun,
} from "../../../scripts/wind-compatibility/reference-comparison.mjs";
import { sha256 } from "../../../scripts/wind-compatibility/reference.mjs";
import { validatePilotAssessmentCases } from "../../../scripts/wind-compatibility/corpus-pilot.mjs";
import { treeDigest } from "../../../scripts/wind-compatibility/differential-runner.mjs";
import { fromRoot } from "../../../scripts/wind-compatibility/reference.mjs";
import {
  assertCurrentCorpusIdentity,
  currentCorpusIdentity,
} from "../../../scripts/wind-compatibility/corpus-identity.mjs";
import { parseCssStructure } from "../../../scripts/wind-compatibility/structure.mjs";
import { canonical } from "../../../scripts/wind-compatibility/corpus-structure.mjs";
import { documentFor } from "../../../scripts/wind-compatibility/browser-adapter.mjs";
import {
  generatedCandidateLists,
  generatedWidths,
  generatedSourceStrings,
} from "../../../scripts/wind-compatibility/corpus-seeds.mjs";

const plan = {
  planId: "plan",
  channel: "stable",
  candidate: {
    package: "tailwindcss",
    version: "4.3.2",
    integrity: "sha512-test",
    source: { status: "unknown" },
  },
  previousAccepted: null,
  inputs: { lock: "lock" },
  toolchain: { node: "test" },
};
const comparison = {
  reportId: "comparison",
  testedSourceSha: "a".repeat(40),
  testedInputs: { digest: "input" },
  profile: { id: "wind-preset-free", version: 1, revision: 2, digest: "profile" },
  candidate: { passing: true, reference: { integrity: "sha512-test", artifactSha256: "artifact" } },
  accepted: null,
  upstreamDelta: [],
  browserDelta: [],
  controlDelta: [],
  windDelta: [],
};
const state = {
  accepted: { schemaVersion: 1, acceptedReference: null },
  reviewed: { schemaVersion: 1, reviewedThrough: null },
};

test("tested closure includes MDX fixtures and excludes only transition records", () => {
  const paths = testedPaths();
  assert.ok(paths.includes("tests/wind-compatibility/extraction/mdx/index.mdx"));
  assert.ok(paths.includes("research/wind-compatibility-profile.md"));
  assert.ok(!paths.includes("tests/wind-compatibility/reference/accepted.json"));
  const a = { schemaVersion: 1, files: [["fixture.mdx", "old"]] };
  a.digest = `sha256:${digest(a.files)}`;
  const b = { schemaVersion: 1, files: [["fixture.mdx", "changed"]] };
  b.digest = `sha256:${digest(b.files)}`;
  assert.equal(sameTestedInputs(a, b), false);
});

test("reviewed rejection advances only reviewed-through; acceptance requires shipping evidence", async () => {
  const classification = {
    schemaVersion: 1,
    kind: "wind-reference-classification",
    planId: "plan",
    comparisonReportId: "comparison",
    disposition: "review-only",
    upstreamChanges: [],
    browserChanges: [],
    controlChanges: [],
    windChanges: [],
    inventoryChanges: [],
  };
  const next = await transition({
    plan,
    assessment: { captureIdentity: "assessment" },
    comparison,
    classification,
    shipping: null,
    state,
    finalSha: "b".repeat(40),
  });
  assert.deepEqual(next.accepted, state.accepted);
  assert.equal(next.reviewed.reviewedThrough.disposition, "review-only");
  await assert.rejects(
    transition({
      plan,
      assessment: { captureIdentity: "assessment" },
      comparison,
      classification: { ...classification, disposition: "accept" },
      shipping: null,
      state,
      finalSha: "b".repeat(40),
    }),
    /Shipping evidence identity, build, or membership invalid/,
  );
});

test("classification rejects omitted or unexplained exact three-way differences", () => {
  const changed = {
    ...comparison,
    upstreamDelta: [
      {
        path: "pilot/x/reference.css",
        acceptedSha256: "old",
        candidateSha256: "new",
        changed: true,
      },
    ],
    browserDelta: [
      {
        id: "pilot/x",
        index: 0,
        engine: "chromium",
        acceptedObservation: { value: "1px" },
        candidateObservation: { value: "2px" },
        changed: true,
      },
    ],
  };
  const base = {
    schemaVersion: 1,
    kind: "wind-reference-classification",
    planId: "plan",
    comparisonReportId: "comparison",
    disposition: "accept",
    upstreamChanges: [],
    browserChanges: [],
    controlChanges: [],
    windChanges: [],
    inventoryChanges: [],
  };
  assert.throws(() => validateClassification(base, changed, plan), /upstream change membership/);
  const classified = {
    ...base,
    upstreamChanges: [
      {
        path: "pilot/x/reference.css",
        acceptedSha256: "old",
        candidateSha256: "new",
        category: "upstream-semantic",
        rationale: "exact change reviewed",
      },
    ],
    browserChanges: [
      {
        id: "pilot/x",
        index: 0,
        engine: "chromium",
        acceptedObservation: { value: "1px" },
        candidateObservation: { value: "2px" },
        category: "unexplained-drift",
        rationale: "pending",
      },
    ],
  };
  assert.throws(() => validateClassification(classified, changed, plan), /Unexplained drift/);
});

test("two-record transition restores on second rename failure and rejects stale replay", async () => {
  const dir = await mkdtemp(join(tmpdir(), "wind-transition-"));
  const acceptedPath = join(dir, "accepted.json"),
    reviewedPath = join(dir, "reviewed.json");
  const opts = {
    acceptedPath,
    reviewedPath,
    lockPath: join(dir, "lock"),
    journalPath: join(dir, "journal.json"),
  };
  const old = {
    accepted: { schemaVersion: 1, acceptedReference: null },
    reviewed: { schemaVersion: 1, reviewedThrough: null },
  };
  const next = {
    accepted: { schemaVersion: 1, acceptedReference: { version: "4.3.2" } },
    reviewed: { schemaVersion: 1, reviewedThrough: { version: "4.3.2" } },
  };
  try {
    await writeFile(acceptedPath, JSON.stringify(old.accepted));
    await writeFile(reviewedPath, JSON.stringify(old.reviewed));
    let calls = 0;
    await assert.rejects(
      writeTransitionAtomically(next, old, {
        ...opts,
        renameFile: async (...args) => {
          if (++calls === 2) throw Error("injected second rename failure");
          return rename(...args);
        },
      }),
      /injected second rename failure/,
    );
    assert.deepEqual(JSON.parse(await readFile(acceptedPath)), old.accepted);
    assert.deepEqual(JSON.parse(await readFile(reviewedPath)), old.reviewed);
    await writeTransitionAtomically(next, old, opts);
    await assert.rejects(writeTransitionAtomically(next, old, opts), /state mismatch/);
    // Simulate a killed writer after replacing only the first record.
    await writeFile(
      opts.journalPath,
      JSON.stringify({
        schemaVersion: 1,
        oldAccepted: JSON.stringify(old.accepted),
        oldReviewed: JSON.stringify(old.reviewed),
      }),
    );
    await withTransitionLock(async () => {}, opts);
    assert.deepEqual(JSON.parse(await readFile(acceptedPath)), old.accepted);
    assert.deepEqual(JSON.parse(await readFile(reviewedPath)), old.reviewed);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("shipping manifest cannot omit a required consumer mechanism or reuse an ID", () => {
  const mechanisms = [
    "source-add",
    "source-edit",
    "source-remove",
    "source-rename",
    "token-change",
    "token-removal",
    "css-build",
    "css-dev",
    "css-ssr",
    "missing-css-detection",
    "component-state",
  ];
  const manifest = {
    schemaVersion: 1,
    kind: "wind-shipping-manifest",
    revision: 1,
    fixtureRoot: "tests/wind-compatibility/shipping/fixtures",
    configFiles: ["tests/wind-compatibility/shipping/config.json"],
    required: mechanisms.map((mechanism) => ({
      id: mechanism,
      mechanism,
      mode: "build",
      engine: "chromium",
      configuration: "semantic",
      fixture: "fixture",
      expectedObservation: { value: "1px" },
      screenshotRequired: false,
    })),
  };
  assert.equal(validateShippingManifest(manifest), true);
  assert.throws(
    () => validateShippingManifest({ ...manifest, required: manifest.required.slice(1) }),
    /incomplete/,
  );
  assert.throws(
    () =>
      validateShippingManifest({
        ...manifest,
        required: manifest.required.map((row) => ({ ...row, id: "same" })),
      }),
    /duplicate/,
  );
});

test("current accepted verification rejects failed and absent run reports", async () => {
  const failed = { passing: false, reports: { pilots: {}, chromium: {}, firefox: {}, webkit: {} } };
  assert.throws(() => requirePassingCurrent(failed), /failed or incomplete/);
  const dir = await mkdtemp(join(tmpdir(), "wind-current-missing-"));
  try {
    await assert.rejects(
      validateRun(dir, { version: "4.3.2" }, {}, { requiredCases: [], requiredControls: [] }, {}),
      /ENOENT/,
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("retained artifact remapping verifies bytes and rejects traversal or symlink escape", async () => {
  const root = await mkdtemp(join(tmpdir(), "wind-bundle-"));
  const output = join(root, "restored"),
    original = "/tmp/original-wind-evidence";
  try {
    await import("node:fs/promises").then((fs) =>
      fs.mkdir(join(output, "pilot"), { recursive: true }),
    );
    const file = join(output, "pilot", "reference.css");
    await writeFile(file, ".block{display:block}");
    const recorded = `${original}/pilot/reference.css`;
    assert.equal(
      (
        await readRecordedArtifact(recorded, sha256(".block{display:block}"), output, original)
      ).toString(),
      ".block{display:block}",
    );
    await assert.rejects(
      readRecordedArtifact(recorded, sha256("changed"), output, original),
      /hash changed/,
    );
    await assert.rejects(
      readRecordedArtifact(
        "/tmp/outside/reference.css",
        sha256(".block{display:block}"),
        output,
        original,
      ),
      /outside recorded origin/,
    );
    await rm(file);
    const outside = join(root, "outside.css");
    await writeFile(outside, ".block{display:block}");
    await symlink(outside, file);
    await assert.rejects(
      readRecordedArtifact(recorded, sha256(".block{display:block}"), output, original),
      /escapes declared bundle/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("assessment raw replay detects changed CSS and a falsely passed browser observation", async () => {
  const root = await mkdtemp(join(tmpdir(), "wind-assessment-raw-"));
  const output = join(root, "pilot");
  const profile = JSON.parse(await readFile(fromRoot("tests/wind-compatibility/profile.json")));
  const observations = JSON.parse(
    await readFile(fromRoot("tests/wind-compatibility/pilot/observations.json")),
  );
  const fixtureDir = fromRoot("tests/wind-compatibility/pilot/block");
  const fixture = await readFile(join(fixtureDir, "case.json"));
  const html = await readFile(join(fixtureDir, "index.html"));
  const windCss = ".block{display:block}",
    referenceCss = ".block { display: block; }";
  const windReport = { inputMode: "compiler", specCaseId: "block" };
  const observe = (side, css) => ({
    engine: side,
    probe: "display",
    verified: true,
    stylesheetSha256: sha256(css),
    servedSha256: sha256(css),
    settings: { documentSha256: sha256(html), authoredCssSha256: sha256("") },
    expected: "block",
    observation: { value: "block" },
    pass: true,
  });
  const row = {
    caseId: "block",
    wind: { cssSha256: sha256(windCss), reportSha256: sha256(JSON.stringify(windReport)) },
    reference: { cssSha256: sha256(referenceCss) },
    fixtureDigest: sha256(fixture),
    fixtureTreeDigest: await treeDigest(fixtureDir),
    configDigest: digest(profile.configurations.empty),
    observations: [
      { wind: observe("wind", windCss), reference: observe("reference", referenceCss) },
    ],
  };
  const report = { cases: [row], controls: [], extraction: [] };
  try {
    await import("node:fs/promises").then((fs) =>
      Promise.all([
        fs.mkdir(join(output, "wind-compiler", "block"), { recursive: true }),
        fs.mkdir(join(output, "reference-compiler", "block"), { recursive: true }),
      ]),
    );
    await writeFile(join(output, "wind-compiler", "block", "wind.css"), windCss);
    await writeFile(
      join(output, "wind-compiler", "block", "report.json"),
      JSON.stringify(windReport),
    );
    await writeFile(join(output, "reference-compiler", "block", "reference.css"), referenceCss);
    const reference = { compile: async () => ({ build: () => referenceCss }) };
    assert.equal(
      await validatePilotAssessmentCases(
        report,
        join(output, "report.json"),
        profile,
        observations,
        reference,
      ),
      true,
    );
    row.observations[0].reference.observation.value = "none";
    await assert.rejects(
      validatePilotAssessmentCases(
        report,
        join(output, "report.json"),
        profile,
        observations,
        reference,
      ),
      /browser evidence invalid/,
    );
    row.observations[0].reference.observation.value = "block";
    await writeFile(join(output, "reference-compiler", "block", "reference.css"), "tampered");
    await assert.rejects(
      validatePilotAssessmentCases(
        report,
        join(output, "report.json"),
        profile,
        observations,
        reference,
      ),
      /raw case changed/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("three-way observation accounting pairs actual flat native control sides", async () => {
  const shape = JSON.parse(
    await readFile(fromRoot("tests/wind-compatibility/reference/native-observation-shape.v1.json")),
  );
  const actual = {
    cases: [],
    controls: [{ controlId: "native-reset-controls", observations: shape.observations }],
  };
  const run = {
    reports: {
      pilots: Object.fromEntries(
        ["chromium", "firefox", "webkit"].map((engine) => [engine, actual]),
      ),
      chromium: { executed: {} },
      firefox: { executed: {} },
      webkit: { executed: {} },
    },
  };
  const rows = observationRows(run);
  for (const engine of ["chromium", "firefox", "webkit"])
    assert.equal(
      rows.filter((row) => row.id === "control/native-reset-controls" && row.engine === engine)
        .length,
      6,
    );
});

test("complete candidate browser mismatch stays assessment data but false pass and missing probe fail", () => {
  const htmlHash = sha256("<div></div>");
  const definitions = [{ name: "display", wind: "block", reference: "block" }];
  const side = (engine, value, pass) => ({
    engine,
    probe: "display",
    verified: true,
    settings: { documentSha256: htmlHash, authoredCssSha256: sha256("") },
    expected: "block",
    observation: { value },
    pass,
  });
  const row = {
    htmlSha256: htmlHash,
    probes: [{ wind: side("wind", "block", true), reference: side("reference", "none", false) }],
  };
  assert.equal(validateCorpusProbeOutcomes(row, definitions, "chromium", htmlHash), true);
  assert.throws(
    () => validateCorpusProbeOutcomes(row, definitions, "chromium", htmlHash, true),
    /outcome invalid/,
  );
  row.probes[0].reference.pass = true;
  assert.throws(
    () => validateCorpusProbeOutcomes(row, definitions, "chromium", htmlHash),
    /outcome invalid/,
  );
  row.probes = [];
  assert.throws(
    () => validateCorpusProbeOutcomes(row, definitions, "chromium", htmlHash),
    /membership/,
  );
});

test("full corpus identity rejects rehashed reports after fixture, helper, config or inventory edits", async () => {
  const root = await mkdtemp(join(tmpdir(), "wind-corpus-identity-"));
  const paths = [
    "scripts/wind-compatibility",
    "tests/wind-compatibility/corpus",
    "tests/wind-compatibility/pilot",
    "tests/wind-compatibility/empty-token",
    "tests/wind-compatibility/extraction",
    "tests/wind-compatibility/profile.json",
    "tests/wind-compatibility/inventory.v1.json",
    "crates/zudo-wind/catalog/zudo-wind-catalog.v1.json",
    "pnpm-lock.yaml",
  ];
  try {
    for (const path of paths) {
      await mkdir(join(root, path, ".."), { recursive: true });
      await cp(fromRoot(path), join(root, path), { recursive: true });
    }
    const manifest = JSON.parse(
      await readFile(join(root, "tests/wind-compatibility/corpus/manifest.json")),
    );
    const profile = JSON.parse(await readFile(join(root, "tests/wind-compatibility/profile.json")));
    const contracts = JSON.parse(
      await readFile(join(root, "tests/wind-compatibility/corpus/structure-contracts.json")),
    );
    const pilotObservations = JSON.parse(
      await readFile(join(root, "tests/wind-compatibility/pilot/observations.json")),
    );
    const executed = {};
    for (const row of manifest.upstreamCases.filter((item) => item.engines.includes("chromium"))) {
      const definition = JSON.parse(
        await readFile(join(root, `tests/wind-compatibility/corpus/upstream/${row.id}/case.json`)),
      );
      executed[`upstream/${row.id}/chromium`] = { configDigest: digest(definition) };
    }
    const inputs = {
      manifest,
      profile,
      contracts,
      pilotObservations,
      executed,
      engine: "chromium",
      source: { sourceArchiveSha256: "source" },
      reference: { identity: { artifactSha256: "artifact" } },
      scanner: { identity: { artifactSha256: "scanner" } },
      windBuild: { gitSha: "a".repeat(40) },
      browserEnvironment: { name: "chromium" },
      rootDir: root,
    };
    const baseline = await currentCorpusIdentity(inputs);
    assert.equal(assertCurrentCorpusIdentity(baseline, baseline), true);
    for (const path of [
      "tests/wind-compatibility/corpus/upstream/display-flex/index.html",
      "scripts/wind-compatibility/corpus-core.mjs",
      "scripts/wind-compatibility/corpus-provenance.mjs",
      "scripts/wind-compatibility/spec-identity.mjs",
      "tests/wind-compatibility/empty-token/configurations.json",
      "tests/wind-compatibility/inventory.v1.json",
    ]) {
      const file = join(root, path),
        original = await readFile(file);
      await writeFile(file, Buffer.concat([original, Buffer.from("\nchanged")]));
      const current = await currentCorpusIdentity(inputs);
      const rehashedReport = {
        identity: baseline,
        reportId: `sha256:${digest({ identity: baseline })}`,
      };
      assert.match(rehashedReport.reportId, /^sha256:/);
      assert.throws(
        () => assertCurrentCorpusIdentity(rehashedReport.identity, current),
        /full current input identity stale/,
      );
      await writeFile(file, original);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("native and supplemental raw rows reject missing or altered retained CSS", async () => {
  const root = await mkdtemp(join(tmpdir(), "wind-aux-raw-"));
  const output = join(root, "restored"),
    original = "/tmp/original-corpus";
  try {
    await mkdir(output);
    const nativeCss = ".block { display: block; }",
      supplementalCss = ".p { padding: 1px; }";
    const nativeFile = join(output, "native.css"),
      windFile = join(output, "wind.css"),
      referenceFile = join(output, "reference.css"),
      inputFile = join(output, "input.css");
    await writeFile(nativeFile, nativeCss);
    await writeFile(windFile, supplementalCss);
    await writeFile(referenceFile, supplementalCss);
    await writeFile(inputFile, "@tailwind utilities;");
    const native = {
      expected: { display: "block" },
      windCssPath: `${original}/native.css`,
      cssSha256: sha256(nativeCss),
      structure: {
        actual: digest(parseCssStructure(nativeCss).map(canonical)),
      },
    };
    const guarantee = { id: "block", writes: { display: "block" } };
    assert.equal(
      (await validateNativeRawRow(native, guarantee, output, original)).toString(),
      nativeCss,
    );
    const probePlan = [
      { candidate: "p", key: "padding", property: "padding-top", role: "target", value: "block" },
    ];
    const documentHtml =
      '<!doctype html><html><head></head><body><div id="box"><span id="target" class="p">x</span><span id="control">x</span></div></body></html>';
    const probe = {
      name: "s/p/padding",
      property: "padding-top",
      selector: "#target",
      wind: "block",
      reference: "block",
      documentHtml,
      authoredCss:
        ":where(#target,#control){padding:24px;margin:0;color:#222;background-color:transparent;}",
    };
    const pair = (engine) => ({
      engine,
      probe: probe.name,
      verified: true,
      stylesheetSha256: sha256(supplementalCss),
      servedSha256: sha256(supplementalCss),
      settings: {
        documentSha256: sha256(documentHtml),
        servedDocumentSha256: sha256(documentFor("p", probe)),
        authoredCssSha256: sha256(probe.authoredCss),
        selector: "#target",
      },
      expected: "block",
      observation: { value: "block" },
      pass: true,
    });
    const supplemental = {
      windCssPath: `${original}/wind.css`,
      windCssSha256: sha256(supplementalCss),
      referenceCssPath: `${original}/reference.css`,
      referenceCssSha256: sha256(supplementalCss),
      referenceInputPath: `${original}/input.css`,
      referenceInputSha256: sha256("@tailwind utilities;"),
      observationCount: 1,
      observations: [[{ wind: pair("wind"), reference: pair("reference") }]],
    };
    const reference = { compile: async () => ({ build: () => supplementalCss }) };
    assert.equal(
      (
        await validateSupplementalRawRow(
          supplemental,
          { caseId: "s", explicitCandidates: ["p"] },
          output,
          original,
          reference,
          probePlan,
        )
      ).wind.toString(),
      supplementalCss,
    );
    supplemental.observations[0][0].reference.expected = "none";
    supplemental.observations[0][0].reference.observation.value = "none";
    await assert.rejects(
      validateSupplementalRawRow(
        supplemental,
        { caseId: "s", explicitCandidates: ["p"] },
        output,
        original,
        reference,
        probePlan,
      ),
      /checked probe changed/,
    );
    supplemental.observations[0][0].reference.expected = "block";
    supplemental.observations[0][0].reference.observation.value = "block";
    await rm(nativeFile);
    await assert.rejects(validateNativeRawRow(native, guarantee, output, original), /ENOENT/);
    await writeFile(referenceFile, "changed");
    await assert.rejects(
      validateSupplementalRawRow(
        supplemental,
        { caseId: "s", explicitCandidates: ["p"] },
        output,
        original,
        reference,
        probePlan,
      ),
      /hash changed/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a native Wind CSS delta blocks acceptance even without a browser observation row", async () => {
  const changed = {
    ...comparison,
    accepted: { passing: true },
    windDelta: [
      {
        path: "corpus-chromium/native-wind/block/wind.css",
        acceptedSha256: "old",
        candidateSha256: "new",
        changed: true,
      },
    ],
  };
  const classification = {
    schemaVersion: 1,
    kind: "wind-reference-classification",
    planId: "plan",
    comparisonReportId: "comparison",
    disposition: "accept",
    upstreamChanges: [],
    browserChanges: [],
    controlChanges: [],
    inventoryChanges: [],
    windChanges: [
      {
        path: "corpus-chromium/native-wind/block/wind.css",
        acceptedSha256: "old",
        candidateSha256: "new",
        category: "local-implementation-gap",
        rationale: "local drift under review",
      },
    ],
  };
  await assert.rejects(
    transition({
      plan,
      assessment: { captureIdentity: "assessment" },
      comparison: changed,
      classification,
      shipping: null,
      state,
      finalSha: "b".repeat(40),
    }),
    /Wind CSS changed/,
  );
});

test("old seeded artifact shape without every raw hash is stale", () => {
  const hash = "a".repeat(64);
  const old = {
    kind: "candidate-permutation",
    id: "specimen-0",
    windActualPath: "/tmp/wind.css",
    windCanonicalPath: "/tmp/canonical-wind.css",
    referenceInputPath: "/tmp/input.css",
    actualPath: "/tmp/actual.css",
    actualSha256: hash,
    canonicalPath: "/tmp/canonical.css",
    canonicalSha256: hash,
  };
  assert.throws(() => validateSeededArtifactHashes(old), /hash missing/);
  const complete = {
    ...old,
    windActualSha256: hash,
    windCanonicalSha256: hash,
    referenceInputSha256: hash,
  };
  assert.equal(validateSeededArtifactHashes(complete), true);
});

test("seeded control requires exact ordered 10/6/6 corpus membership", async () => {
  const policy = JSON.parse(
    await readFile(fromRoot("tests/wind-compatibility/corpus/seed-regressions.json")),
  );
  const specimens = [...policy.permanentSpecimens, ...generatedCandidateLists()];
  const widths = generatedWidths(),
    sources = generatedSourceStrings();
  const scannerIdentity = { package: "oxide-test" };
  const seeded = {
    outcome: "matched",
    seed: policy.seed,
    permanentSpecimens: policy.permanentSpecimens.length,
    generatedSpecimens: policy.generatedCount,
    generatedArbitraryWidths: widths,
    generatedSourceStrings: policy.generatedSourceCount,
    scannerIdentity,
    artifacts: [
      ...specimens.map((candidates, index) => ({
        kind: "candidate-permutation",
        id: `specimen-${index}`,
        candidates,
        canonical: [...new Set(candidates)].sort(),
      })),
      ...widths.map((width, index) => ({
        kind: "arbitrary-width",
        id: `generated-${index}`,
        candidate: `w-[${width}px]`,
      })),
      ...sources.map((source, index) => ({
        kind: "source-string",
        id: `source-${index}`,
        sourceSha256: sha256(source),
      })),
    ],
  };
  assert.equal(validateSeededMembership(seeded, policy, scannerIdentity), true);
  assert.throws(
    () =>
      validateSeededMembership(
        { ...seeded, artifacts: seeded.artifacts.slice(1) },
        policy,
        scannerIdentity,
      ),
    /membership/,
  );
  assert.throws(
    () =>
      validateSeededMembership(
        { ...seeded, artifacts: [seeded.artifacts[0], ...seeded.artifacts.slice(0, -1)] },
        policy,
        scannerIdentity,
      ),
    /membership/,
  );
  const substituted = seeded.artifacts.map((row) => ({ ...row }));
  substituted.at(-1).sourceSha256 = sha256("substituted source");
  assert.throws(
    () => validateSeededMembership({ ...seeded, artifacts: substituted }, policy, scannerIdentity),
    /source input changed/,
  );
});

test("corpus and pilot must identify the same browser executable", () => {
  const pilot = {
    name: "chromium",
    executable: "/tmp/browser",
    executableSha256: "a".repeat(64),
    revision: "1228",
  };
  assert.equal(assertSameBrowserEnvironment(pilot, pilot, "chromium"), true);
  assert.throws(
    () =>
      assertSameBrowserEnvironment(
        { ...pilot, executableSha256: "b".repeat(64) },
        pilot,
        "chromium",
      ),
    /browser executable/,
  );
  assert.throws(
    () => assertSameBrowserEnvironment({ ...pilot, executable: "/tmp/other" }, pilot, "chromium"),
    /browser executable/,
  );
});

test("upstream-only review requires exact assessed change membership and cannot claim acceptance", () => {
  const assessment = {
    captureIdentity: "capture",
    sections: {
      changelog: { releaseVersions: ["4.3.2"], releaseIds: [17], releaseBodiesSha256: ["body"] },
      artifacts: {
        changes: [{ path: "dist/lib.mjs", before: null, after: { sha256: "new", bytes: 12 } }],
      },
      source: {
        changes: [{ path: "src/utilities.ts", before: null, after: { sha256: "new", bytes: 13 } }],
      },
      tests: {
        changes: [
          { path: "src/utilities.test.ts", before: null, after: { sha256: "new", bytes: 14 } },
        ],
      },
    },
  };
  const classified = (row) => ({
    ...row,
    category: "upstream-semantic",
    rationale: "reviewed exact delta",
  });
  const record = {
    schemaVersion: 1,
    kind: "wind-upstream-review",
    planId: "plan",
    assessmentIdentity: "capture",
    disposition: "review-only",
    reason: "upstream-only-review",
    independentCompatibility: false,
    rationale: "comparison not attempted",
    classification: {
      releases: [classified({ version: "4.3.2", id: 17, bodySha256: "body" })],
      artifacts: assessment.sections.artifacts.changes.map(classified),
      source: assessment.sections.source.changes.map(classified),
      tests: assessment.sections.tests.changes.map(classified),
    },
  };
  assert.equal(validateUpstreamReview(record, assessment, plan), true);
  assert.throws(
    () =>
      validateUpstreamReview(
        { ...record, classification: { ...record.classification, tests: [] } },
        assessment,
        plan,
      ),
    /membership/,
  );
  assert.throws(
    () => validateUpstreamReview({ ...record, disposition: "accept" }, assessment, plan),
    /cannot claim acceptance/,
  );
  assert.throws(
    () =>
      validateUpstreamReview(
        { ...record, reason: "unsupported-reference-execution" },
        assessment,
        plan,
      ),
    /cannot claim acceptance/,
  );
  const next = upstreamReviewTransition({
    plan,
    assessment,
    classification: record,
    state,
    finalSha: "b".repeat(40),
    testedInputs: { digest: "inputs" },
  });
  assert.deepEqual(next.accepted, state.accepted);
  assert.equal(next.reviewed.reviewedThrough.independentCompatibility, false);
});

test("reviewed-through preserves the highest version while accepted may advance below it", () => {
  const higher = {
    schemaVersion: 1,
    reviewedThrough: { version: "4.4.0", disposition: "review-only" },
  };
  const lower = { schemaVersion: 1, reviewedThrough: { version: "4.3.3", disposition: "accept" } };
  const equal = { schemaVersion: 1, reviewedThrough: { version: "4.4.0", disposition: "accept" } };
  assert.deepEqual(monotonicReviewed(higher, lower), higher);
  assert.deepEqual(monotonicReviewed(higher, equal), equal);
  assert.deepEqual(monotonicReviewed({ schemaVersion: 1, reviewedThrough: null }, lower), lower);
});

test("review-only write preserves accepted bytes and readers refuse an interrupted transition", async () => {
  const dir = await mkdtemp(join(tmpdir(), "wind-review-only-"));
  const acceptedPath = join(dir, "accepted.json"),
    reviewedPath = join(dir, "reviewed.json");
  const opts = {
    acceptedPath,
    reviewedPath,
    lockPath: join(dir, "lock"),
    journalPath: join(dir, "journal.json"),
  };
  const accepted = { schemaVersion: 1, acceptedReference: null };
  const reviewed = { schemaVersion: 1, reviewedThrough: null };
  const originalBytes = '{ "schemaVersion": 1, "acceptedReference": null }\n';
  try {
    await writeFile(acceptedPath, originalBytes);
    await writeFile(reviewedPath, JSON.stringify(reviewed));
    await writeTransitionAtomically(
      { accepted, reviewed: { schemaVersion: 1, reviewedThrough: { version: "4.3.2" } } },
      { accepted, reviewed },
      opts,
    );
    assert.equal(await readFile(acceptedPath, "utf8"), originalBytes);
    await writeFile(
      opts.journalPath,
      JSON.stringify({
        schemaVersion: 1,
        oldAccepted: originalBytes,
        oldReviewed: JSON.stringify(reviewed),
      }),
    );
    await assert.rejects(assertStableTransition(opts), /recovery required/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
