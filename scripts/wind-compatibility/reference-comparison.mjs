import { spawnSync } from "node:child_process";
import { mkdir, readFile, readdir, writeFile, realpath } from "node:fs/promises";
import { resolve, relative } from "node:path";
import { checkMutations, completeCorpus, expectedObligations, expectedOutcomes } from "./corpus-core.mjs";
import { digest, fromRoot, outsideCheckout, readJson, sha256 } from "./reference.mjs";
import { canonical } from "./corpus-structure.mjs";
import { parseCssStructure } from "./structure.mjs";
import { testedInputIdentity } from "./reference-identity.mjs";
import { matchesExpected } from "./browser-adapter.mjs";
import { currentPilotIdentity, validatePilotArtifacts, validatePilotEnvelope,
  validatePilotAssessmentArtifacts, validatePilotAssessmentEnvelope } from "./corpus-pilot.mjs";
import { loadReference } from "./differential-runner.mjs";
import { loadIndependentScanner } from "./oxide-scanner.mjs";
import { candidateInventory } from "./reference-inventory.mjs";
import { assertCurrentCorpusIdentity, currentCorpusIdentity } from "./corpus-identity.mjs";

const engines = ["chromium", "firefox", "webkit"];
const requiredTargetedKeys = Object.freeze({
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
    "control/wrong-value-detection", "control/missing-rule-detection",
  ],
});
const shaReport = (report) => {
  const { reportId: _id, ...body } = report;
  return `sha256:${digest(body)}`;
};
const same = (a, b, label) => {
  if (digest(a) !== digest(b)) throw Error(`${label} differs from reviewed membership`);
};

export async function rawTree(directory) {
  const files = [];
  async function visit(path) {
    for (const entry of await readdir(path, { withFileTypes: true })) {
      const child = resolve(path, entry.name);
      if (entry.isDirectory()) await visit(child);
      else if (entry.isFile()) files.push([relative(directory, child), sha256(await readFile(child))]);
      else throw Error(`Nonregular comparison artifact: ${child}`);
    }
  }
  await visit(directory);
  return files.sort((a, b) => a[0].localeCompare(b[0]));
}

export async function readRecordedArtifact(path, hash, output, originalOutput) {
  const origin = resolve(originalOutput);
  if (!path || !resolve(path).startsWith(`${origin}/`))
    throw Error(`Raw artifact path outside recorded origin: ${path}`);
  const actual = resolve(output, relative(origin, resolve(path)));
  const physical = await realpath(actual), physicalRoot = await realpath(output);
  if (!physical.startsWith(`${physicalRoot}/`))
    throw Error(`Raw artifact escapes declared bundle: ${path}`);
  const bytes = await readFile(physical);
  if (hash && sha256(bytes) !== hash) throw Error(`Raw artifact hash changed: ${path}`);
  return bytes;
}

function expectedCorpusIds(manifest, engine) {
  return expectedObligations(manifest).filter((id) => id.endsWith(`/${engine}`));
}

export function validateCorpusProbeOutcomes(row, definitions, engine, htmlHash,
  strictArtifacts = false) {
  if (!Array.isArray(row.probes) || row.probes.length !== definitions.length ||
      row.htmlSha256 !== htmlHash)
    throw Error(`${engine} browser probe membership/HTML changed`);
  for (let index = 0; index < definitions.length; index++) {
    const probe = definitions[index], pair = row.probes[index];
    for (const sideName of ["wind", "reference"]) {
      const item = pair?.[sideName];
      if (!item || item.probe !== probe.name || item.engine !== sideName ||
          item.settings?.documentSha256 !== htmlHash ||
          item.settings?.authoredCssSha256 !== sha256(probe.authoredCss ?? "") ||
          digest(item.expected) !== digest(probe[sideName]) ||
          item.pass !== matchesExpected(item.observation, item.expected) ||
          (strictArtifacts && item.pass !== true))
        throw Error(`${engine} browser probe identity or outcome invalid: ${probe.name}`);
    }
  }
  return true;
}

export async function validateNativeRawRow(row, guarantee, output, originalOutput) {
  if (!row || digest(row.expected) !== digest(guarantee.writes))
    throw Error(`Native guarantee claim missing or changed: ${guarantee.id}`);
  const css = await readRecordedArtifact(row.windCssPath, row.cssSha256, output, originalOutput);
  if (row.structure?.actual !== digest(parseCssStructure(css.toString("utf8")).map(canonical)))
    throw Error(`Native guarantee raw structure changed: ${guarantee.id}`);
  return css;
}

export async function validateSupplementalRawRow(claim, definition, output,
  originalOutput, referenceCompiler = null) {
  if (!claim) throw Error(`Supplemental claim missing: ${definition.caseId}`);
  const wind = await readRecordedArtifact(claim.windCssPath, claim.windCssSha256,
    output, originalOutput);
  const referenceCss = await readRecordedArtifact(claim.referenceCssPath,
    claim.referenceCssSha256, output, originalOutput);
  const referenceInput = await readRecordedArtifact(claim.referenceInputPath,
    claim.referenceInputSha256, output, originalOutput);
  if (referenceCompiler) {
    const compiler = await referenceCompiler.compile(referenceInput.toString("utf8"));
    if (compiler.build(definition.explicitCandidates) !== referenceCss.toString("utf8"))
      throw Error(`Supplemental reference compiler output changed: ${definition.caseId}`);
  }
  const pairs = claim.observations?.flat();
  if (!Array.isArray(pairs) || pairs.length !== claim.observationCount ||
      pairs.some((pair) => [pair.wind, pair.reference].some((side) =>
        !side?.verified || side.stylesheetSha256 !== side.servedSha256 ||
        side.stylesheetSha256 !== (side === pair.wind
          ? sha256(wind) : sha256(referenceCss)) ||
        side.pass !== matchesExpected(side.observation, side.expected))))
    throw Error(`Supplemental raw browser observation changed: ${definition.caseId}`);
  return { wind, referenceCss, referenceInput };
}

export function validateSeededArtifactHashes(artifact) {
  const paths = Object.keys(artifact).filter((key) => key.endsWith("Path"));
  if (!paths.length) throw Error("Seeded artifact has no retained raw paths");
  for (const key of paths) {
    const hashKey = `${key.slice(0, -4)}Sha256`;
    if (!/^[0-9a-f]{64}$/.test(artifact[hashKey] ?? ""))
      throw Error(`Seeded raw artifact hash missing: ${artifact.kind}/${artifact.id}/${key}`);
  }
  return true;
}

export async function validateRun(output, expectedReference, input, profile, manifest,
  { cache = null, strictArtifacts = false, originalOutput = output } = {}) {
  output = await outsideCheckout(output);
  const pilots = {};
  for (const engine of engines) {
    const pilot = await readJson(resolve(output, `pilot-${engine}/report.json`));
    if (pilot.reportId !== shaReport(pilot) || pilot.kind !== "wind-differential-pilot" ||
        pilot.admission !== "pilot-only" || pilot.infrastructure ||
        pilot.browserAdmission || ![0, 1].includes(pilot.exitCode) ||
        pilot.identity.browserEnvironment.name !== engine)
      throw Error(`Pilot ${engine} infrastructure failure or edited report`);
    same(pilot.pilotCaseIds, profile.requiredCases.map((row) => row.id), `${engine} pilot cases`);
    same(pilot.cases.map((row) => row.caseId), pilot.pilotCaseIds,
      `${engine} pilot executed cases`);
    same(pilot.controls.map((row) => row.controlId), profile.requiredControls,
      `${engine} pilot controls`);
    same(pilot.extraction.map((row) => row.caseId),
      (await readJson(fromRoot("tests/wind-compatibility/extraction/manifest.json"))).caseIds,
      `${engine} pilot extraction`);
    if (pilot.cases.some((row) => !row.outcome || !row.checks ||
        Object.keys(row.checks).length !== 4) ||
        pilot.controls.some((row) => !row.outcome) ||
        pilot.extraction.some((row) => !row.outcome))
      throw Error(`${engine} pilot observation missing`);
    pilots[engine] = pilot;
  }
  const reports = { pilots };
  for (const engine of engines) {
    for (const row of pilots[engine].cases) {
      const pilotDir = resolve(output, `pilot-${engine}`);
      const originalPilotDir = resolve(originalOutput, `pilot-${engine}`);
      await readRecordedArtifact(resolve(originalPilotDir, "wind-compiler", row.caseId, "wind.css"),
        row.wind?.cssSha256, pilotDir, originalPilotDir);
      await readRecordedArtifact(resolve(originalPilotDir, "reference-compiler", row.caseId,
        "reference.css"), row.reference?.cssSha256, pilotDir, originalPilotDir);
      if (!Array.isArray(row.observations) || !row.observations.length ||
          row.observations.some((pair) => !pair.wind?.verified || !pair.reference?.verified ||
            pair.wind.stylesheetSha256 !== pair.wind.servedSha256 ||
            pair.reference.stylesheetSha256 !== pair.reference.servedSha256 ||
            pair.wind.stylesheetSha256 !== row.wind.cssSha256 ||
            pair.reference.stylesheetSha256 !== row.reference.cssSha256))
        throw Error(`${engine} pilot raw observation/transport invalid: ${row.caseId}`);
    }
  }
  for (const engine of engines) {
    const report = await readJson(resolve(output, `corpus-${engine}/report.json`));
    if (report.reportId !== shaReport(report) || report.engine !== engine ||
        report.kind !== "wind-independent-corpus" || report.infrastructure ||
        ![0, 1].includes(report.exitCode))
      throw Error(`Corpus ${engine} infrastructure failure or edited report`);
    const expected = expectedCorpusIds(manifest, engine);
    same(report.expected, expected, `${engine} obligations`);
    same(Object.keys(report.executed).sort(), [...expected].sort(), `${engine} executions`);
    const accounting = completeCorpus(expected, report.executed, expectedOutcomes(manifest, profile));
    same(report.accounting, accounting, `${engine} accounting`);
    const pilot = pilots[engine];
    if (report.controls?.pilotReportId !== pilot.reportId ||
        !report.controls?.pilotReportPath?.endsWith(`/pilot-${engine}/report.json`) ||
        !report.controls?.pilot ||
        digest(Object.keys(report.controls.pilot)) !== digest(profile.requiredControls))
      throw Error(`${engine} pilot control import incomplete`);
    for (const row of pilot.controls)
      same(report.controls.pilot[row.controlId], row, `${engine} pilot control ${row.controlId}`);
    if (!Array.isArray(report.differenceSummary) ||
        report.differenceSummary.length !== manifest.reviewedDifferences.length)
      throw Error(`${engine} difference membership missing`);
    for (const difference of report.differenceSummary) {
      const policy = manifest.reviewedDifferences.find((row) => row.id === difference.id);
      if (!policy) throw Error(`${engine} unreviewed difference ${difference.id}`);
      const requiredCaseIds = policy.caseIds.filter((id) =>
        manifest.upstreamCases.find((row) => row.id === id)?.engines.includes(engine));
      const observedCaseIds = requiredCaseIds.filter((id) =>
        report.executed[`upstream/${id}/${engine}`]?.reviewedDifferenceIds?.includes(difference.id));
      same(difference.requiredCaseIds, requiredCaseIds, `${engine} difference requirements`);
      same(difference.observedCaseIds, observedCaseIds, `${engine} observed differences`);
      if (difference.pass !== (digest(requiredCaseIds) === digest(observedCaseIds)))
        throw Error(`${engine} difference pass claim invalid`);
    }
    reports[engine] = report;
  }
  const reference = pilots.chromium.identity.reference;
  if (reference.package !== "tailwindcss" || reference.version !== expectedReference.version ||
      reference.integrity !== expectedReference.integrity ||
      reference.artifactSha256 !== expectedReference.artifactSha256 ||
      !reference.moduleGraphSha256 ||
      engines.some((engine) => digest(reports[engine].identity.reference) !== digest(reference) ||
        digest(pilots[engine].identity.reference) !== digest(reference)))
    throw Error("Reference identity differs across executed runs");
  const windBuild = pilots.chromium.identity.windBuild;
  if (!/^[0-9a-f]{40}$/.test(windBuild.gitSha) ||
      engines.some((engine) => digest(reports[engine].identity.windBuild) !== digest(windBuild) ||
        digest(pilots[engine].identity.windBuild) !== digest(windBuild)))
    throw Error("Wind build identity differs across executed runs");
  if (engines.some((engine) => pilots[engine].identity.profileDigest !== input.profile ||
      reports[engine].identity.profileDigest !== input.profile))
    throw Error("Profile identity differs from current input");
  for (const engine of engines) {
    if (reports[engine].identity.browserEnvironment?.requiredMatrixMember !== true ||
        pilots[engine].identity.browserEnvironment?.requiredMatrixMember !== true)
      throw Error(`${engine} browser environment outside required matrix`);
    const member = profile.browserPolicy?.requiredMatrix?.find((row) => row.browser === engine);
    const environment = reports[engine].identity.browserEnvironment;
    if (!member || environment.hostPlatform !== member.hostPlatform ||
        environment.version !== member.browserVersion ||
        environment.revision !== member.revision)
      throw Error(`${engine} browser identity differs from profile matrix`);
  }
  const targeted = profile.browserPolicy?.targetedObligations ?? [];
  if (digest(Object.keys(manifest.targetedExecutionKeys ?? {})) !== digest(targeted) ||
      digest(manifest.targetedExecutionKeys) !== digest(requiredTargetedKeys))
    throw Error("Targeted browser execution mapping missing or unreviewed");
  for (const engine of engines.filter((name) => name !== "chromium"))
    for (const id of targeted) {
      const keys = manifest.targetedExecutionKeys[id];
      if (!Array.isArray(keys) || !keys.length || new Set(keys).size !== keys.length ||
          keys.some((key) => key.startsWith("pilot/")
            ? !Object.hasOwn(reports[engine].executed, `${key}/${engine}`)
            : key.startsWith("control/")
              ? reports[engine].controls.pilot[key.slice("control/" )]?.outcome !== "matched"
              : true))
        throw Error(`${engine} profile targeted obligation absent: ${id}`);
    }
  const passing = engines.every((engine) => pilots[engine].complete === true &&
    pilots[engine].exitCode === 0) &&
    engines.every((engine) => reports[engine].complete === true && reports[engine].exitCode === 0);
  let referenceCompiler = null, scanner = null;
  if (cache) {
    referenceCompiler = await loadReference(cache, expectedReference);
    scanner = await loadIndependentScanner(cache);
    const pilotManifest = await readJson(fromRoot("tests/wind-compatibility/pilot/manifest.json"));
    const observations = await readJson(fromRoot("tests/wind-compatibility/pilot/observations.json"));
    const extraction = await readJson(fromRoot("tests/wind-compatibility/extraction/manifest.json"));
    for (const engine of engines) {
      const pilot = pilots[engine];
      const expectedIdentity = await currentPilotIdentity({ profile, manifest: pilotManifest,
        windBuild, reference: referenceCompiler, scanner,
        browserEnvironment: pilot.identity.browserEnvironment });
      validatePilotAssessmentEnvelope(pilot, profile, pilotManifest, observations,
        extraction, expectedIdentity);
      await validatePilotAssessmentArtifacts(pilot,
        resolve(output, `pilot-${engine}/report.json`), profile, observations,
        referenceCompiler, scanner);
      if (strictArtifacts) {
        if (!passing) throw Error("Passing complete report required for acceptance replay");
        validatePilotEnvelope(pilot, profile, pilotManifest, observations, extraction, expectedIdentity);
        await validatePilotArtifacts(pilot, resolve(output, `pilot-${engine}/report.json`),
          profile, pilotManifest, observations, extraction, referenceCompiler, scanner);
      }
    }
  } else if (strictArtifacts) throw Error("Reference cache required for acceptance replay");
  for (const engine of engines) {
    const corpusDir = resolve(output, `corpus-${engine}`);
    const originalCorpusDir = resolve(originalOutput, `corpus-${engine}`);
    if (engine === "chromium") {
      const empty = await readJson(fromRoot("tests/wind-compatibility/empty-token/manifest.json"));
      for (const guarantee of empty.nativeGuarantees) {
        const id = `native/${guarantee.id}/chromium`;
        const row = reports[engine].executed[id];
        await validateNativeRawRow(row, guarantee, corpusDir, originalCorpusDir);
      }
      for (const row of empty.cases.filter((item) => manifest.supplementalCaseIds.includes(item.caseId))) {
        const id = `supplemental/${row.caseId}/chromium`;
        const claim = reports[engine].executed[id];
        await validateSupplementalRawRow(claim, row, corpusDir, originalCorpusDir,
          referenceCompiler);
      }
      const controls = reports[engine].controls;
      same(controls.mutations, checkMutations(controls.mutations?.validControls,
        controls.mutations?.mutations), "corpus mutation controls");
      const seeded = controls.seeded;
      if (!Array.isArray(seeded?.artifacts) || !seeded.artifacts.length ||
          seeded.outcome !== "matched")
        throw Error("Seeded corpus controls missing or failed");
      for (const artifact of seeded.artifacts) {
        validateSeededArtifactHashes(artifact);
        for (const [key, value] of Object.entries(artifact))
          if (key.endsWith("Path")) {
            const hashKey = `${key.slice(0, -4)}Sha256`;
            await readRecordedArtifact(value, artifact[hashKey],
              corpusDir, originalCorpusDir);
          }
      }
    }
    for (const [id, row] of Object.entries(reports[engine].executed)) {
      if (!id.startsWith("upstream/")) continue;
      const raw = {};
      for (const [pathKey, hashKey] of [["windCssPath", "windCssSha256"],
        ["referenceCssPath", "referenceCssSha256"],
        ["referenceInputPath", "referenceInputSha256"]])
        raw[pathKey] = await readRecordedArtifact(row[pathKey], row[hashKey],
          corpusDir, originalCorpusDir);
      if (cache) {
        const fixture = manifest.upstreamCases.find((item) =>
          id === `upstream/${item.id}/${engine}`);
        if (!fixture) throw Error(`${engine} upstream fixture absent for ${id}`);
        const definition = await readJson(fromRoot(
          `tests/wind-compatibility/corpus/upstream/${fixture.id}/case.json`));
        if (row.configDigest !== digest(definition) ||
            row.originalInput !== fixture.originalInput ||
            row.source !== `${fixture.upstreamPath}#${fixture.upstreamTest}`)
          throw Error(`${engine} corpus fixture/config/source claim stale: ${id}`);
        const compiler = await referenceCompiler.compile(raw.referenceInputPath.toString("utf8"));
        if (compiler.build(fixture.candidates) !== raw.referenceCssPath.toString("utf8"))
          throw Error(`${engine} raw upstream compiler output changed: ${id}`);
      }
      if (!Array.isArray(row.probes) || !row.probes.length ||
          row.probes.some((pair) => [pair.wind, pair.reference].some((side) =>
            !side?.verified || side.stylesheetSha256 !== side.servedSha256 ||
            side.stylesheetSha256 !== (side === pair.wind
              ? row.windCssSha256 : row.referenceCssSha256))))
        throw Error(`${engine} browser transport invalid: ${id}`);
      const fixtureId = id.split("/")[1];
      const fixtureRoot = fromRoot(`tests/wind-compatibility/corpus/upstream/${fixtureId}`);
      const html = await readFile(resolve(fixtureRoot, "index.html"));
      const definitions = (await readJson(resolve(fixtureRoot, "probes.json")))
        .filter((probe) => !probe.engines || probe.engines.includes(engine));
      validateCorpusProbeOutcomes(row, definitions, engine, sha256(html), strictArtifacts);
    }
    if (cache) {
      const archive = await readFile(resolve(cache, `response-${manifest.sourceArchiveSha256}`));
      if (sha256(archive) !== manifest.sourceArchiveSha256)
        throw Error("Pinned imported-test source archive changed");
      const source = { sourceArchiveSha256: manifest.sourceArchiveSha256,
        sourceTagCommit: manifest.sourceTagCommit,
        packageLink: manifest.sourceToPackageLink };
      const contracts = await readJson(fromRoot(
        "tests/wind-compatibility/corpus/structure-contracts.json"));
      const pilotObservations = await readJson(fromRoot(
        "tests/wind-compatibility/pilot/observations.json"));
      const expectedIdentity = await currentCorpusIdentity({ manifest, profile, contracts,
        pilotObservations, executed: reports[engine].executed, engine, source,
        reference: referenceCompiler, scanner, windBuild,
        browserEnvironment: reports[engine].identity.browserEnvironment });
      assertCurrentCorpusIdentity(reports[engine].identity, expectedIdentity);
    }
  }
  return { reports, windBuild, reference, passing, files: await rawTree(output) };
}

export function requirePassingCurrent(run) {
  if (!run || run.passing !== true || !run.reports?.pilots ||
      engines.some((engine) => run.reports.pilots[engine]?.complete !== true ||
        run.reports.pilots[engine]?.exitCode !== 0 ||
        run.reports[engine]?.complete !== true || run.reports[engine]?.exitCode !== 0))
    throw Error("Current accepted reference verification failed or incomplete");
  return true;
}

export async function executeReferenceRun({ output, referenceFile, binary, buildManifest, cache,
  assessmentMode = false }) {
  output = await outsideCheckout(output);
  await mkdir(output, { recursive: true });
  const common = ["--wind-binary", binary, "--wind-build-manifest", buildManifest,
    "--cache", cache, "--reference", referenceFile];
  const run = (script, args) => {
    const result = spawnSync(process.execPath, [fromRoot(script), ...common, ...args],
      { cwd: fromRoot("."), stdio: "inherit", timeout: 30 * 60 * 1000 });
    if (result.error || ![0, 1].includes(result.status))
      throw Error(`${script} could not produce complete evidence: ${result.error?.message ?? result.status}`);
  };
  for (const engine of engines) {
    run("scripts/wind-compatibility/differential-runner.mjs",
      ["--output", resolve(output, `pilot-${engine}`), "--engine", engine]);
    run("scripts/wind-compatibility/corpus-runner.mjs",
      ["--output", resolve(output, `corpus-${engine}`), "--engine", engine,
        "--pilot-report", resolve(output, `pilot-${engine}/report.json`),
        ...(assessmentMode ? ["--assessment-mode", "yes"] : [])]);
  }
}

export function observationRows(run) {
  const rows = [];
  const add = (id, pairs, engine, configDigest, fixtureDigest) => {
    for (let index = 0; index < (pairs ?? []).length; index++) {
      const pair = pairs[index];
      if (!pair?.wind?.verified || !pair?.reference?.verified ||
          pair.wind.servedSha256 !== pair.wind.stylesheetSha256 ||
          pair.reference.servedSha256 !== pair.reference.stylesheetSha256)
        throw Error(`Three-way browser observation incomplete: ${id}/${index}`);
      rows.push({ id, index, engine, configDigest, fixtureDigest,
        wind: pair.wind.observation, reference: pair.reference.observation,
        windSettings: pair.wind.settings, referenceSettings: pair.reference.settings,
        windCssSha256: pair.wind.stylesheetSha256,
        referenceCssSha256: pair.reference.stylesheetSha256 });
    }
  };
  for (const engine of engines) {
    for (const row of run.reports.pilots[engine].cases)
      add(`pilot/${row.caseId}`, row.observations, engine, row.configDigest, row.fixtureTreeDigest);
    for (const row of run.reports.pilots[engine].controls) {
      if (!row.observations) continue;
      let pairs = row.observations;
      if (row.controlId === "native-reset-controls") {
        if (pairs.length % 2 !== 0) throw Error(`${engine} native observations truncated`);
        pairs = Array.from({ length: pairs.length / 2 }, (_, index) => {
          const wind = row.observations[2 * index], reference = row.observations[2 * index + 1];
          if (wind.engine !== "wind" || reference.engine !== "reference" ||
              wind.probe !== reference.probe)
            throw Error(`${engine} native observation side order changed`);
          return { wind, reference };
        });
      }
      add(`control/${row.controlId}`, pairs, engine, null, row.fixtureTreeDigest ?? null);
    }
  }
  for (const engine of engines)
    for (const [key, row] of Object.entries(run.reports[engine].executed))
      if (Array.isArray(row.probes)) add(key, row.probes, engine, row.configDigest,
        row.htmlSha256 ?? null);
      else if (Array.isArray(row.observations))
        add(key, row.observations.flat(), engine, row.configDigest ?? null,
          row.htmlSha256 ?? null);
  return rows;
}

function runBrowserIdentity(run, engine) {
  return run.reports[engine].identity.browserEnvironment;
}

export async function compareOutputs({ plan, assessment, output, candidate, accepted, input,
  cache = null, strictArtifacts = false, artifactOrigin = output }) {
  const profile = await readJson(fromRoot("tests/wind-compatibility/profile.json"));
  const manifest = await readJson(fromRoot("tests/wind-compatibility/corpus/manifest.json"));
  const candidateRun = await validateRun(resolve(output, "candidate"), candidate, input,
    profile, manifest, { cache, strictArtifacts,
      originalOutput: resolve(artifactOrigin, "candidate") });
  const acceptedRun = accepted && await validateRun(resolve(output, "accepted"), accepted,
    input, profile, manifest, { cache, strictArtifacts,
      originalOutput: resolve(artifactOrigin, "accepted") });
  if (acceptedRun && digest(candidateRun.windBuild) !== digest(acceptedRun.windBuild))
    throw Error("Three-way runs used different Wind builds");
  const oldFiles = new Map((acceptedRun?.files ?? []).filter(([path]) => path.endsWith("reference.css")));
  const newFiles = new Map(candidateRun.files.filter(([path]) => path.endsWith("reference.css")));
  if (acceptedRun) same([...oldFiles.keys()], [...newFiles.keys()], "three-way reference CSS membership");
  const upstreamDelta = [...newFiles].map(([path, hash]) => ({
    path, acceptedSha256: oldFiles.get(path) ?? null, candidateSha256: hash,
    changed: acceptedRun ? oldFiles.get(path) !== hash : null,
  }));
  const oldWindFiles = new Map((acceptedRun?.files ?? []).filter(([path]) =>
    path.endsWith("wind.css")));
  const newWindFiles = new Map(candidateRun.files.filter(([path]) => path.endsWith("wind.css")));
  if (acceptedRun) same([...oldWindFiles.keys()], [...newWindFiles.keys()],
    "three-way Wind CSS membership");
  const windDelta = [...newWindFiles].map(([path, hash]) => ({ path,
    acceptedSha256: oldWindFiles.get(path) ?? null, candidateSha256: hash,
    changed: acceptedRun ? oldWindFiles.get(path) !== hash : null }));
  const candidateObservations = observationRows(candidateRun);
  const acceptedObservations = acceptedRun && observationRows(acceptedRun);
  if (acceptedObservations) same(candidateObservations.map((row) =>
    [row.id, row.index, row.engine, row.configDigest, row.fixtureDigest]),
    acceptedObservations.map((row) =>
      [row.id, row.index, row.engine, row.configDigest, row.fixtureDigest]),
    "three-way browser case/config/fixture membership");
  const browserDelta = candidateObservations.map((row, index) => {
    const old = acceptedObservations?.[index];
    const candidateEnvironment = runBrowserIdentity(candidateRun, row.engine);
    const acceptedEnvironment = acceptedRun && runBrowserIdentity(acceptedRun, row.engine);
    const localChanged = old ? digest(row.wind) !== digest(old.wind) ||
      digest(row.windSettings) !== digest(old.windSettings) ||
      row.windCssSha256 !== old.windCssSha256 ||
      digest(candidateEnvironment) !== digest(acceptedEnvironment) ||
      digest(row.referenceSettings) !== digest(old.referenceSettings) : null;
    return { id: row.id, index: row.index, engine: row.engine,
      configDigest: row.configDigest, fixtureDigest: row.fixtureDigest,
      browserEnvironment: candidateEnvironment,
      acceptedBrowserEnvironment: acceptedEnvironment,
      windObservation: row.wind, acceptedWindObservation: old?.wind ?? null,
      windCssSha256: row.windCssSha256,
      acceptedWindCssSha256: old?.windCssSha256 ?? null,
      localChanged,
      acceptedObservation: old?.reference ?? null,
      candidateObservation: row.reference,
      acceptedCssSha256: old?.referenceCssSha256 ?? null,
      candidateCssSha256: row.referenceCssSha256,
      changed: old ? digest(old.reference) !== digest(row.reference) : null };
  });
  const controlDelta = engines.flatMap((engine) =>
    candidateRun.reports.pilots[engine].controls.map((row) => {
      const old = acceptedRun?.reports.pilots[engine].controls.find((item) =>
        item.controlId === row.controlId);
      return { id: row.controlId, engine, acceptedDigest: old ? `sha256:${digest(old)}` : null,
        candidateDigest: `sha256:${digest(row)}`,
        acceptedOutcome: old?.outcome ?? null, candidateOutcome: row.outcome,
        changed: old ? digest(old) !== digest(row) : null };
    }));
  const body = {
    schemaVersion: 1, kind: "wind-three-way-comparison", planId: plan.planId,
    artifactOrigin,
    assessmentIdentity: assessment.captureIdentity,
    testedSourceSha: candidateRun.windBuild.gitSha,
    testedInputs: await testedInputIdentity(),
    profile: { id: profile.profileId, version: profile.profileVersion,
      revision: profile.profileRevision, digest: input.profile },
    candidate: { reference: candidateRun.reference, windBuild: candidateRun.windBuild,
      passing: candidateRun.passing, reports: Object.fromEntries([
        ...engines.map((engine) => [`pilot-${engine}`, candidateRun.reports.pilots[engine].reportId]),
        ...engines.map((engine) => [`corpus-${engine}`, candidateRun.reports[engine].reportId])]),
      files: candidateRun.files },
    accepted: acceptedRun && { reference: acceptedRun.reference, passing: acceptedRun.passing,
      reports: Object.fromEntries([
        ...engines.map((engine) => [`pilot-${engine}`, acceptedRun.reports.pilots[engine].reportId]),
        ...engines.map((engine) => [`corpus-${engine}`, acceptedRun.reports[engine].reportId])]),
      files: acceptedRun.files },
    upstreamDelta,
    windDelta,
    browserDelta,
    controlDelta,
    candidateInventory: plan.previousAccepted
      ? await candidateInventory({ candidate, accepted, assessment, cache }) : null,
    inventoryMembership: plan.previousAccepted
      ? "candidate-inventory-refreshed" : "bootstrap-pinned-inventory",
    admission: "pending-independent-review-and-shipping-evidence",
  };
  return { ...body, reportId: `sha256:${digest(body)}` };
}
