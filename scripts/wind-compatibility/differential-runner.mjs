#!/usr/bin/env node
import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, readFile, writeFile, readdir, stat, unlink, realpath } from "node:fs/promises";
import { basename, dirname, extname, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { homedir } from "node:os";
import { gunzipSync } from "node:zlib";
import { browserIdentity, observePair, observeIsolated } from "./browser-adapter.mjs";
import {
  artifactIdentity,
  classify,
  compareExtractionSets,
  completeReport,
  outcomes,
  validatePilot,
} from "./differential-core.mjs";
import { compareStructure, expectedExtractionStructure, parseCssStructure } from "./structure.mjs";
import { loadIndependentScanner, scanOriginal } from "./oxide-scanner.mjs";
import {
  digest,
  fromRoot,
  outsideCheckout,
  readJson,
  sha256,
  tarPackageIdentity,
} from "./reference.mjs";

const defaults = {
  fixture: fromRoot("tests/wind-compatibility/pilot"),
  extraction: fromRoot("tests/wind-compatibility/extraction"),
  cache: "/tmp/zfb-wind-reference-cache",
};
const referenceSha = "3673da9004404d12d4672bb5d002945319c2b3dea8c13ea022fdd33a3e260e62";

function args(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i]?.startsWith("--") || !argv[i + 1]) throw Error("Expected --key value pairs");
    options[argv[i].slice(2)] = argv[i + 1];
  }
  for (const key of ["wind-binary", "wind-build-manifest", "output"])
    if (!options[key]) throw Error(`Missing --${key}`);
  return { ...defaults, ...options };
}

async function treeDigest(directory) {
  const entries = [];
  async function visit(path) {
    for (const entry of await readdir(path, { withFileTypes: true })) {
      const child = resolve(path, entry.name);
      if (entry.isDirectory()) await visit(child);
      else if (entry.isFile())
        entries.push([child.slice(directory.length + 1), sha256(await readFile(child))]);
    }
  }
  await visit(directory);
  return digest(entries.sort((a, b) => a[0].localeCompare(b[0])));
}

export async function verifyWindBuild(binary, manifest) {
  const identity = await readJson(manifest);
  const gitSha = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: fromRoot("."),
    encoding: "utf8",
  }).trim();
  const sourceDigest = await treeDigest(fromRoot("crates/zudo-wind/src"));
  const exampleDigest = sha256(
    await readFile(fromRoot("crates/zudo-wind/examples/wind_fixture_css.rs")),
  );
  const assetsDigest = await treeDigest(fromRoot("crates/zudo-wind/assets"));
  const crateManifestDigest = sha256(await readFile(fromRoot("crates/zudo-wind/Cargo.toml")));
  const lockDigest = sha256(await readFile(fromRoot("Cargo.lock")));
  const binaryDigest = sha256(await readFile(binary));
  if (
    resolve(binary) !== resolve(identity.binaryPath) ||
    (await realpath(binary)) !== (await realpath(identity.binaryPath))
  )
    throw Error("Wind binary path does not match successful Cargo artifact");
  if (
    identity.schemaVersion !== 1 ||
    identity.gitSha !== gitSha ||
    identity.sourceDigest !== sourceDigest ||
    identity.exampleDigest !== exampleDigest ||
    identity.assetsDigest !== assetsDigest ||
    identity.crateManifestDigest !== crateManifestDigest ||
    identity.lockDigest !== lockDigest ||
    identity.binarySha256 !== binaryDigest ||
    identity.buildCommand !==
      "cargo build --locked -p zudo-wind --example wind_fixture_css --message-format=json" ||
    identity.buildExitCode !== 0
  )
    throw Error("Wind binary/source build identity stale or unverified");
  if (
    (await stat(binary)).mtimeMs <
    (await stat(fromRoot("crates/zudo-wind/examples/wind_fixture_css.rs"))).mtimeMs
  )
    throw Error("Wind binary predates example source");
  return identity;
}

export async function writeWindBuildManifest(binary, output, buildMode) {
  output = await outsideCheckout(output);
  const build = {
    schemaVersion: 1,
    gitSha: execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: fromRoot("."),
      encoding: "utf8",
    }).trim(),
    sourceDigest: await treeDigest(fromRoot("crates/zudo-wind/src")),
    exampleDigest: sha256(
      await readFile(fromRoot("crates/zudo-wind/examples/wind_fixture_css.rs")),
    ),
    assetsDigest: await treeDigest(fromRoot("crates/zudo-wind/assets")),
    crateManifestDigest: sha256(await readFile(fromRoot("crates/zudo-wind/Cargo.toml"))),
    lockDigest: sha256(await readFile(fromRoot("Cargo.lock"))),
    binarySha256: sha256(await readFile(binary)),
    binaryPath: await realpath(binary),
    buildCommand:
      "cargo build --locked -p zudo-wind --example wind_fixture_css --message-format=json",
    buildMode,
    buildExitCode: 0,
    recordedAt: new Date().toISOString(),
  };
  await writeFile(output, JSON.stringify(build, null, 2) + "\n");
  return build;
}

function tarMember(bytes, filename) {
  const tar = gunzipSync(bytes);
  for (let at = 0; at + 512 <= tar.length;) {
    const name = tar.toString("utf8", at, at + 100).replace(/\0.*$/, "");
    if (!name) break;
    const size = parseInt(
      tar
        .toString("ascii", at + 124, at + 136)
        .replace(/\0.*$/, "")
        .trim(),
      8,
    );
    if (!Number.isFinite(size)) throw Error("Invalid reference archive");
    if (name === filename) return tar.subarray(at + 512, at + 512 + size);
    at += 512 + Math.ceil(size / 512) * 512;
  }
  throw Error(`Reference archive member absent: ${filename}`);
}

export async function loadReference(cache, bootstrap) {
  const archive = await outsideCheckout(resolve(cache, `response-${referenceSha}`));
  const bytes = await readFile(archive);
  if (
    sha256(bytes) !== referenceSha ||
    "sha512-" + createHash("sha512").update(bytes).digest("base64") !== bootstrap.integrity ||
    JSON.stringify(tarPackageIdentity(bytes)) !==
      JSON.stringify({ name: "tailwindcss", version: "4.3.2" })
  )
    throw Error("Pinned Tailwind tarball identity mismatch");
  const moduleBytes = tarMember(bytes, "package/dist/lib.mjs");
  const preflightBytes = tarMember(bytes, "package/preflight.css");
  const modulePath = await outsideCheckout(
    resolve(cache, `tailwindcss-4.3.2-${referenceSha}/dist/lib.mjs`),
  );
  if (sha256(await readFile(modulePath)) !== sha256(moduleBytes))
    throw Error("Reference module differs from tarball");
  const { compile } = await import(pathToFileURL(modulePath).href);
  if (typeof compile !== "function") throw Error("Reference compile API absent");
  return {
    compile,
    preflightCss: preflightBytes.toString("utf8"),
    identity: {
      package: "tailwindcss",
      version: "4.3.2",
      integrity: bootstrap.integrity,
      artifactSha256: referenceSha,
      moduleSha256: sha256(moduleBytes),
      preflightSha256: sha256(preflightBytes),
      source: bootstrap.source,
    },
  };
}

function referenceTheme(profile, row) {
  const config = profile.configurations[row.configuration];
  if (!config || config.wind.reset !== "none")
    throw Error(`Unsupported shared configuration ${row.configuration}`);
  const declarations = Object.entries(config.referenceTheme)
    .map(([name, value]) => {
      if (!/^--[a-z0-9-]+$/.test(name) || !/^[#a-zA-Z0-9.%-]+$/.test(value))
        throw Error("Invalid theme mapping");
      return `${name}:${value};`;
    })
    .join(" ");
  return `@theme { --*: initial; ${declarations} } @tailwind utilities;`;
}

async function runWind(binary, root, output, mode) {
  const command = [root, output, "--mode", mode, "--manifest", resolve(root, "manifest.json")];
  const run = spawnSync(binary, command, { encoding: "utf8", timeout: 60_000 });
  if (run.error || run.status !== 0)
    throw Error(`Wind ${mode} failed: ${run.error?.message ?? run.stderr ?? run.status}`);
  return { status: run.status, stderr: run.stderr, stdout: run.stdout };
}

function expectedWindConfig(row, fixture, profile) {
  const config = profile.configurations[row.configuration].wind;
  const allowed = new Set([
    "caseId",
    "reset",
    "explicitCandidates",
    "colors",
    "spacing",
    "spacingUnit",
    "breakpoints",
  ]);
  for (const key of Object.keys(fixture))
    if (!allowed.has(key))
      throw Error(`Shared fixture contains unreviewed configuration ${row.id}/${key}`);
  if (
    fixture.reset !== config.reset ||
    digest(fixture.spacing ?? {}) !== digest(config.tokens.spacing ?? {}) ||
    digest(fixture.colors ?? {}) !== digest(config.tokens.colors ?? {}) ||
    (fixture.spacingUnit ?? null) !== (config.tokens.spacingUnit ?? null) ||
    digest(fixture.breakpoints ?? {}) !== digest(config.breakpoints ?? {})
  )
    throw Error(`Wind fixture config drift: ${row.id}`);
  if (fixture.caseId !== row.id || digest(fixture.explicitCandidates) !== digest([row.candidate]))
    throw Error(`Candidate fixture drift: ${row.id}`);
}

function structuralChecks(row, windCss, referenceCss, windReport, diagnosticsPass) {
  const expectWind =
    row.implementation === "implemented" &&
    !["unconfigured-p-4", "undeclared-palette"].includes(row.id);
  const reportRule =
    windReport.rules.length === Number(expectWind) &&
    windReport.rules.includes(row.candidate) === expectWind;
  const parsed = compareStructure(
    row.id,
    windCss,
    referenceCss,
    row.reviewedDifferenceIds,
    diagnosticsPass,
  );
  return {
    pass: reportRule && parsed.pass,
    reportRule,
    ...parsed,
  };
}

export function diagnosticCheck(row, report) {
  const diagnostics = report.diagnostics ?? [];
  const expected = {
    "unconfigured-p-4": {
      code: "ZW006",
      rejectionId: "R17",
      message: "nonzero numeric spacing requires spacingUnit",
    },
    "undeclared-palette": {
      code: "ZW006",
      rejectionId: "R17",
      message: "unknown value or token gray-500",
    },
    "contents-gap": {
      code: "ZW014",
      rejectionId: null,
      messagePrefix: "unsupported foreign utility (migration vocabulary",
    },
  }[row.id];
  if (!expected) return diagnostics.length === 0;
  if (diagnostics.length !== 1) return false;
  const diagnostic = diagnostics[0];
  return (
    diagnostic.code === expected.code &&
    diagnostic.severity === "error" &&
    diagnostic.candidate === row.candidate &&
    diagnostic.rejectionId === expected.rejectionId &&
    diagnostic.suggestion === null &&
    diagnostic.origin?.kind === "manifest" &&
    diagnostic.origin.producer === "wind-fixture-css" &&
    diagnostic.origin.path === `${row.id}/case.json` &&
    diagnostic.origin.index === 0 &&
    (expected.message
      ? diagnostic.message === expected.message
      : diagnostic.message.startsWith(expected.messagePrefix) &&
        diagnostic.message.includes("Tailwind `contents`"))
  );
}

async function runExtraction(binary, reference, scanner, extractionRoot, output) {
  const manifest = await readJson(resolve(extractionRoot, "manifest.json"));
  const ids = manifest.caseIds;
  if (!Array.isArray(ids) || ids.length === 0 || new Set(ids).size !== ids.length)
    throw Error("Invalid extraction manifest");
  const actual = (await readdir(extractionRoot, { withFileTypes: true }))
    .filter((x) => x.isDirectory())
    .map((x) => x.name)
    .sort();
  if (digest(actual) !== digest([...ids].sort()))
    throw Error("Extraction fixture manifest incomplete");
  const windOutput = resolve(output, "wind-extraction");
  const extractionProcess = await runWind(binary, extractionRoot, windOutput, "extract");
  const rows = [];
  for (const id of ids) {
    const dir = resolve(extractionRoot, id),
      definition = await readJson(resolve(dir, "case.json"));
    if ((definition.explicitCandidates ?? []).length)
      throw Error(`Extraction ${id} has explicit candidates`);
    const sourceName = definition.sourceFile,
      extension = extname(sourceName).slice(1);
    const source = await readFile(resolve(dir, sourceName), "utf8");
    const expected = definition.expectedCandidates;
    const assertions = await readJson(resolve(dir, "assertions.json"));
    if (
      !Array.isArray(expected) ||
      expected.length === 0 ||
      ![
        "expectedWindCandidates",
        "expectedReferenceCandidates",
        "expectedWindRules",
        "expectedReferenceRules",
      ].every(
        (key) =>
          Array.isArray(assertions[key]) &&
          new Set(assertions[key]).size === assertions[key].length,
      )
    )
      throw Error(`Extraction ${id} missing expected candidates`);
    const referenceCandidates = scanOriginal(scanner.Scanner, source, extension);
    const windReport = await readJson(resolve(windOutput, id, "report.json"));
    const windCandidates = windReport.extractedCandidates;
    const sourceInputDigest = sha256(source);
    const windCss = await readFile(resolve(windOutput, id, "wind.css"), "utf8");
    const compiler = await reference.compile("@theme { --*: initial; } @tailwind utilities;");
    const referenceCss = compiler.build(referenceCandidates);
    const windStructure = parseCssStructure(windCss);
    const referenceStructure = parseCssStructure(referenceCss);
    const cssRuleNames = (tree) =>
      tree
        .filter((node) => node.kind === "rule" && node.head.startsWith("."))
        .map((node) => node.head.slice(1).replaceAll("\\:", ":"));
    const pass =
      compareExtractionSets(
        {
          windCandidates,
          referenceCandidates,
          windRules: windReport.rules,
          referenceRules: cssRuleNames(referenceStructure),
        },
        assertions,
      ) &&
      digest(cssRuleNames(windStructure).sort()) ===
        digest([...assertions.expectedWindRules].sort()) &&
      digest(windStructure) ===
        digest(expectedExtractionStructure(assertions.expectedWindRules, "wind")) &&
      digest(referenceStructure) ===
        digest(expectedExtractionStructure(assertions.expectedReferenceRules, "reference")) &&
      expected.every(
        (candidate) =>
          assertions.expectedWindCandidates.includes(candidate) &&
          assertions.expectedReferenceCandidates.includes(candidate),
      ) &&
      windReport.inputMode === "extract" &&
      windReport.sourceBytes === Buffer.byteLength(source);
    rows.push({
      caseId: id,
      outcome: pass ? outcomes.shared : outcomes.mismatch,
      expected,
      windCandidates,
      referenceCandidates,
      assertions,
      windStructure,
      referenceStructure,
      sourceInputDigest,
      fixtureTreeDigest: await treeDigest(dir),
      windCssSha256: sha256(windCss),
      referenceCssSha256: sha256(referenceCss),
      windReportSha256: sha256(JSON.stringify(windReport)),
      windProcessStatus: extractionProcess.status,
    });
  }
  return rows;
}

async function main() {
  const options = args(process.argv.slice(2));
  options.output = await outsideCheckout(options.output);
  await mkdir(options.output, { recursive: true });
  const profile = await readJson(fromRoot("tests/wind-compatibility/profile.json"));
  const windLib = await readFile(fromRoot("crates/zudo-wind/src/lib.rs"), "utf8");
  if (
    !windLib.includes("pub const SPEC_VERSION: u32 = 1;") ||
    !windLib.includes("pub const SPEC_REVISION: u32 = 12;")
  )
    throw Error("Wind spec identity differs from pilot adapter");
  const manifest = await readJson(resolve(options.fixture, "manifest.json"));
  const observations = await readJson(resolve(options.fixture, "observations.json"));
  const required = validatePilot(profile, manifest, observations);
  const fixtureDirs = (await readdir(options.fixture, { withFileTypes: true }))
    .filter((x) => x.isDirectory())
    .map((x) => x.name)
    .sort();
  if (digest(fixtureDirs) !== digest([...manifest.caseIds].sort()))
    throw Error("Pilot fixture manifest incomplete");
  const windBuild = await verifyWindBuild(options["wind-binary"], options["wind-build-manifest"]);
  const bootstrap = await readJson(fromRoot("tests/wind-compatibility/reference/bootstrap.json"));
  const reference = await loadReference(options.cache, bootstrap);
  const scanner = await loadIndependentScanner(options.cache);
  const windOutput = resolve(options.output, "wind-compiler");
  const processResult = await runWind(
    options["wind-binary"],
    options.fixture,
    windOutput,
    "compiler",
  );
  const { chromium } = await import("@playwright/test");
  const browser = await chromium.launch({ headless: true });
  const rows = {},
    controls = {};
  let environment;
  try {
    environment = await browserIdentity(browser, chromium.executablePath());
    if (
      environment.playwrightVersion !== profile.browserPolicy.playwrightTestVersion ||
      environment.playwrightCoreVersion !== profile.browserPolicy.playwrightCoreVersion ||
      environment.browserManifestSha256 !== profile.browserPolicy.browserManifest.sha256
    )
      throw Error("Browser toolchain differs from profile pin");
    environment.requiredMatrixMember = profile.browserPolicy.requiredMatrix.some(
      (member) =>
        member.os === environment.platform &&
        member.browser === environment.name &&
        member.revision === environment.revision &&
        member.browserVersion === environment.version,
    );
    let missingRejected = false;
    try {
      validatePilot(profile, { caseIds: [...manifest.caseIds, "not-a-fixture"] }, observations);
    } catch {
      missingRejected = true;
    }
    controls["missing-fixture-rejection"] = {
      controlId: "missing-fixture-rejection",
      outcome: missingRejected ? outcomes.shared : outcomes.mismatch,
    };
    const stalePath = resolve(options.output, "stale-wind-build.json");
    await writeFile(stalePath, JSON.stringify({ ...windBuild, sourceDigest: "0".repeat(64) }));
    let staleRejected = false;
    try {
      await verifyWindBuild(options["wind-binary"], stalePath);
    } catch {
      staleRejected = true;
    } finally {
      await unlink(stalePath);
    }
    controls["stale-input-rejection"] = {
      controlId: "stale-input-rejection",
      outcome: staleRejected ? outcomes.shared : outcomes.mismatch,
    };
    for (const id of manifest.caseIds) {
      const row = required.get(id),
        fixture = await readJson(resolve(options.fixture, id, "case.json"));
      const probeHtml = await readFile(resolve(options.fixture, id, "index.html"), "utf8");
      const probes = observations[id].probes.map((probe) => ({
        ...probe,
        documentHtml: probeHtml,
      }));
      expectedWindConfig(row, fixture, profile);
      const config = profile.configurations[row.configuration];
      const compiler = await reference.compile(referenceTheme(profile, row));
      const referenceCss = compiler.build([row.candidate]);
      const windCss = await readFile(resolve(windOutput, id, "wind.css"), "utf8");
      const windReport = await readJson(resolve(windOutput, id, "report.json"));
      const diagnosticsPass = diagnosticCheck(row, windReport);
      const structure = structuralChecks(row, windCss, referenceCss, windReport, diagnosticsPass);
      const observation = await observePair(browser, windCss, referenceCss, row.candidate, probes);
      const observed = observation.every((pair) => pair.wind.pass && pair.reference.pass);
      const checks = {
        identity: windReport.inputMode === "compiler" && windReport.specCaseId === id,
        structure: structure.pass,
        observations: observed,
        diagnostics: diagnosticsPass,
      };
      rows[id] = {
        caseId: id,
        outcome: classify(row, checks),
        disposition: row.disposition,
        reviewedDifferenceIds: structure.observedDifferenceIds,
        checks,
        structure,
        observations: observation,
        configDigest: digest(config),
        sourceInputDigest: digest([row.candidate]),
        fixtureDigest: sha256(await readFile(resolve(options.fixture, id, "case.json"))),
        fixtureTreeDigest: await treeDigest(resolve(options.fixture, id)),
        wind: artifactIdentity(windCss, windReport),
        reference: { cssSha256: sha256(referenceCss) },
      };
      await mkdir(resolve(options.output, "reference-compiler", id), { recursive: true });
      await writeFile(
        resolve(options.output, "reference-compiler", id, "reference.css"),
        referenceCss,
      );
      if (id === "block") {
        const probe = probes[0];
        const missing = await observeIsolated(browser, "", row.candidate, probe, "wind");
        const wrong = await observeIsolated(
          browser,
          windCss.replace(/display:\s*block/, "display: none"),
          row.candidate,
          probe,
          "wind",
        );
        const wrongSelectorCss = windCss.replace(/\.block\s*\{/, ".wrong { ");
        const wrongSelector = await observeIsolated(
          browser,
          wrongSelectorCss,
          row.candidate,
          probe,
          "wind",
        );
        controls["missing-rule-detection"] = {
          controlId: "missing-rule-detection",
          outcome:
            !missing.pass && wrongSelectorCss !== windCss && !wrongSelector.pass
              ? outcomes.shared
              : outcomes.mismatch,
          mutations: {
            removedStylesheetDetected: !missing.pass,
            wrongSelectorDetected: !wrongSelector.pass,
          },
        };
        controls["wrong-value-detection"] = {
          controlId: "wrong-value-detection",
          outcome: !wrong.pass ? outcomes.shared : outcomes.mismatch,
        };
        controls["empty-output-accounting"] = {
          controlId: "empty-output-accounting",
          outcome:
            !structure.pass || windCss.length === 0 || referenceCss.length === 0
              ? outcomes.mismatch
              : outcomes.shared,
        };
        const injected = await observeIsolated(
          browser,
          `${windCss}\n.injected { color: red; }`,
          row.candidate,
          probe,
          "wind",
        );
        controls["extra-rule-detection"] = {
          controlId: "extra-rule-detection",
          outcome:
            injected.verified &&
            !compareStructure(
              "block",
              `${windCss}\n.injected { color: red; }`,
              referenceCss,
              row.reviewedDifferenceIds,
              true,
            ).pass
              ? outcomes.shared
              : outcomes.mismatch,
        };
        const authored = {
          ...probe,
          name: "authored-cascade",
          authoredCss: ".block { display: inline; }",
          wind: "inline",
          reference: "inline",
        };
        const cascades = await observePair(browser, windCss, referenceCss, row.candidate, [
          authored,
        ]);
        controls["specificity-and-authored-cascade"] = {
          controlId: "specificity-and-authored-cascade",
          outcome: cascades.every((x) => x.wind.pass && x.reference.pass)
            ? outcomes.shared
            : outcomes.mismatch,
          observations: cascades,
        };
      }
      if (id === "hover-block") {
        const probe = probes[1];
        const wrongMediaCss = windCss.replace(/\(hover:\s*hover\)/, "(hover: none)");
        const wrongMedia = await observeIsolated(
          browser,
          wrongMediaCss,
          row.candidate,
          probe,
          "wind",
        );
        controls["variant-inactive-state"] = {
          controlId: "variant-inactive-state",
          outcome:
            observation[0]?.wind.pass &&
            observation[0]?.reference.pass &&
            observation[1]?.wind.pass &&
            observation[1]?.reference.pass &&
            wrongMediaCss !== windCss &&
            !wrongMedia.pass
              ? outcomes.shared
              : outcomes.mismatch,
          mutations: { wrongMediaDetected: wrongMediaCss !== windCss && !wrongMedia.pass },
        };
      }
      if (id === "mx-auto") {
        const byName = new Map(probes.map((probe, index) => [probe.name, observation[index]]));
        const verticalDifferences = ["ltr", "rtl"].every((direction) => {
          const left = byName.get(`vertical-rl-${direction}-geometry-left`);
          const top = byName.get(`vertical-rl-${direction}-geometry-top`);
          return (
            Math.abs(left.wind.observation.number - left.reference.observation.number) > 10 ||
            Math.abs(top.wind.observation.number - top.reference.observation.number) > 10
          );
        });
        controls["writing-mode-axis"] = {
          controlId: "writing-mode-axis",
          outcome:
            probes.length === 24 &&
            verticalDifferences &&
            observation.every((x) => x.wind.pass && x.reference.pass)
              ? outcomes.shared
              : outcomes.mismatch,
          verticalDifferences,
        };
      }
      if (id === "named-spacing") {
        const scoped = {
          name: "nested-token-scope",
          property: "padding-top",
          scopeVars: "--zw-spacing-hsp-sm:31px;--spacing-hsp-sm:31px;",
          wind: "31px",
          reference: "31px",
          documentHtml: probeHtml,
        };
        const scope = await observePair(browser, windCss, referenceCss, row.candidate, [scoped]);
        controls["nested-token-scope"] = {
          controlId: "nested-token-scope",
          outcome: scope.every((x) => x.wind.pass && x.reference.pass)
            ? outcomes.shared
            : outcomes.mismatch,
          observations: scope,
        };
      }
    }
    const nativeRoot = fromRoot("tests/wind-compatibility/native");
    const nativeOutput = resolve(options.output, "wind-native");
    const nativeProcess = await runWind(
      options["wind-binary"],
      nativeRoot,
      nativeOutput,
      "compiler",
    );
    const nativeCss = await readFile(resolve(nativeOutput, "native-reset", "wind.css"), "utf8");
    const nativeReport = await readJson(resolve(nativeOutput, "native-reset", "report.json"));
    const nativeAuthored = await readFile(
      resolve(nativeRoot, "native-reset", "authored.css"),
      "utf8",
    );
    const nativeHtml = await readFile(resolve(nativeRoot, "native-reset", "index.html"), "utf8");
    const nativeReferenceCompiler = await reference.compile(
      "@theme { --*: initial; } @tailwind utilities;",
    );
    const nativeReferenceCss = `@layer theme, base, components, utilities;\n@layer base {\n${reference.preflightCss}\n}\n@layer utilities {\n${nativeReferenceCompiler.build(["block"])}\n}\n${nativeAuthored}`;
    await mkdir(resolve(options.output, "reference-native"), { recursive: true });
    await writeFile(
      resolve(options.output, "reference-native", "reference.css"),
      nativeReferenceCss,
    );
    const resetProbe = {
      name: "native-reset-box-sizing",
      property: "box-sizing",
      wind: "border-box",
      reference: "border-box",
      documentHtml: nativeHtml,
    };
    const authoredProbe = {
      name: "native-authored-cascade",
      property: "display",
      wind: "inline",
      reference: "inline",
      documentHtml: nativeHtml,
    };
    const nativeProbes = [
      resetProbe,
      authoredProbe,
      {
        name: "native-heading",
        selector: "#heading",
        property: "font-size",
        wind: "32px",
        reference: "16px",
        documentHtml: nativeHtml,
      },
      {
        name: "native-list",
        selector: "#list",
        property: "list-style-type",
        wind: "disc",
        reference: "none",
        documentHtml: nativeHtml,
      },
      {
        name: "native-border",
        selector: "#border",
        property: "border-top-style",
        wind: "inset",
        reference: "solid",
        documentHtml: nativeHtml,
      },
      {
        name: "native-form",
        selector: "#form",
        property: "border-top-style",
        wind: "outset",
        reference: "solid",
        documentHtml: nativeHtml,
      },
    ];
    const nativeResult = [];
    for (const probe of nativeProbes) {
      nativeResult.push(await observeIsolated(browser, nativeCss, "block native", probe, "wind"));
      nativeResult.push(
        await observeIsolated(browser, nativeReferenceCss, "block native", probe, "reference"),
      );
    }
    controls["native-reset-controls"] = {
      controlId: "native-reset-controls",
      outcome:
        nativeCss.includes("@layer zw-reset") &&
        nativeCss.includes(".native") &&
        nativeReferenceCss.includes("box-sizing: border-box") &&
        nativeReport.inputMode === "compiler" &&
        nativeResult.every((x) => x.pass)
          ? outcomes.shared
          : outcomes.mismatch,
      scope: "separate native reset and authored CSS behavior; no shared-lane equivalence claim",
      referenceComposition:
        "pinned preflight and compiled utility in the package index.css layer order; default theme omitted",
      observations: nativeResult,
      windCssSha256: sha256(nativeCss),
      referenceCssSha256: sha256(nativeReferenceCss),
      fixtureTreeDigest: await treeDigest(nativeRoot),
      windProcessStatus: nativeProcess.status,
    };
  } finally {
    await browser.close();
  }
  const extraction = await runExtraction(
    options["wind-binary"],
    reference,
    scanner,
    options.extraction,
    options.output,
  );
  const identity = {
    windBuild,
    windSpecVersion: 1,
    windSpecRevision: 12,
    catalogDigest: await treeDigest(fromRoot("crates/zudo-wind/src/catalog")),
    reference: reference.identity,
    scanner: scanner.identity,
    profileDigest: sha256(await readFile(fromRoot("tests/wind-compatibility/profile.json"))),
    fixtureManifestDigest: sha256(await readFile(resolve(options.fixture, "manifest.json"))),
    pilotFixtureTreeDigest: await treeDigest(options.fixture),
    extractionFixtureTreeDigest: await treeDigest(options.extraction),
    nativeFixtureTreeDigest: await treeDigest(fromRoot("tests/wind-compatibility/native")),
    sourceInputDigest: digest(manifest.caseIds.map((id) => [id, required.get(id).candidate])),
    observationsDigest: sha256(await readFile(resolve(options.fixture, "observations.json"))),
    assertionDigest: sha256(await readFile(resolve(options.fixture, "observations.json"))),
    extractionManifestDigest: sha256(await readFile(resolve(options.extraction, "manifest.json"))),
    adapterDigest: digest([
      sha256(await readFile(import.meta.filename)),
      sha256(await readFile(fromRoot("scripts/wind-compatibility/browser-adapter.mjs"))),
      sha256(await readFile(fromRoot("scripts/wind-compatibility/differential-core.mjs"))),
      sha256(await readFile(fromRoot("scripts/wind-compatibility/structure.mjs"))),
      sha256(await readFile(fromRoot("scripts/wind-compatibility/oxide-scanner.mjs"))),
      sha256(await readFile(fromRoot("scripts/wind-compatibility/reference.mjs"))),
    ]),
    lockfileDigest: sha256(await readFile(fromRoot("pnpm-lock.yaml"))),
    browserEnvironment: environment,
    resetMode: { shared: "none", native: "minimal-v1", referenceNative: "pinned-preflight-css" },
    caseIds: manifest.caseIds,
    windProcess: processResult,
  };
  const report = completeReport(profile, manifest, rows, controls, identity);
  report.extraction = extraction;
  if (extraction.some((row) => row.outcome !== outcomes.shared)) report.exitCode = 1;
  if (!environment.requiredMatrixMember) {
    report.complete = false;
    report.exitCode = 1;
    report.browserAdmission = "unverified-host-outside-profile-matrix";
  }
  const { reportId: _oldId, ...signedBody } = report;
  report.reportId = `sha256:${digest(signedBody)}`;
  await writeFile(resolve(options.output, "report.json"), JSON.stringify(report, null, 2) + "\n");
  process.stdout.write(`${resolve(options.output, "report.json")}\n`);
  process.exitCode = report.exitCode;
}

async function infrastructureReport(error, argv) {
  let options;
  try {
    options = args(argv);
  } catch {
    return;
  }
  const output = await outsideCheckout(options.output);
  await mkdir(output, { recursive: true });
  const profile = await readJson(fromRoot("tests/wind-compatibility/profile.json"));
  const manifest = await readJson(resolve(options.fixture, "manifest.json"));
  const report = completeReport(
    profile,
    manifest,
    {},
    {},
    {
      sourceGitSha: execFileSync("git", ["rev-parse", "HEAD"], {
        cwd: fromRoot("."),
        encoding: "utf8",
      }).trim(),
      profileDigest: sha256(await readFile(fromRoot("tests/wind-compatibility/profile.json"))),
      fixtureManifestDigest: sha256(await readFile(resolve(options.fixture, "manifest.json"))),
    },
  );
  report.infrastructure = { outcome: outcomes.infrastructure, message: error.message };
  report.complete = false;
  report.exitCode = 1;
  const { reportId: _oldId, ...signedBody } = report;
  report.reportId = `sha256:${digest(signedBody)}`;
  await writeFile(resolve(output, "report.json"), JSON.stringify(report, null, 2) + "\n");
}

if (process.argv[1] && resolve(process.argv[1]) === import.meta.filename) {
  if (process.argv[2] === "build-wind") {
    const manifestPath = process.argv[3];
    if (!manifestPath || process.argv.length !== 4)
      throw Error("Usage: build-wind <manifest-path>");
    const buildArgs = [
      "build",
      "--locked",
      "-p",
      "zudo-wind",
      "--example",
      "wind_fixture_css",
      "--message-format=json",
    ];
    const ci = process.env.CI === "true";
    const executable = ci ? "cargo" : "bash";
    const commandArgs = ci
      ? buildArgs
      : [resolve(homedir(), ".codex/scripts/heavy-guard.sh"), "--", "cargo", ...buildArgs];
    let run;
    for (let attempt = 0; ; attempt++) {
      run = spawnSync(executable, commandArgs, {
        cwd: fromRoot("."),
        encoding: "utf8",
        maxBuffer: 20 * 1024 * 1024,
        timeout: 0,
      });
      if (run.stdout) process.stdout.write(run.stdout);
      if (run.stderr) process.stderr.write(run.stderr);
      const verdict = /verdict=(\S+)/.exec(`${run.stdout ?? ""}\n${run.stderr ?? ""}`)?.[1];
      if (!ci && (run.status === 75 || verdict === "ENV_SUSPECT") && attempt < 1) continue;
      break;
    }
    if (!ci && /verdict=ENV_SUSPECT/.test(`${run.stdout ?? ""}\n${run.stderr ?? ""}`))
      throw Error("Guarded build remained environment-suspect; defer to CI");
    if (run.error || run.status !== 0)
      throw Error(`Guarded Wind build failed: ${run.error?.message ?? run.status}`);
    const artifacts = run.stdout
      .split(/\r?\n/)
      .flatMap((line) => {
        try {
          return [JSON.parse(line)];
        } catch {
          return [];
        }
      })
      .filter(
        (message) =>
          message.reason === "compiler-artifact" &&
          message.target?.name === "wind_fixture_css" &&
          message.target?.kind?.includes("example") &&
          message.executable,
      );
    if (artifacts.length !== 1)
      throw Error(`Expected one Cargo example artifact, got ${artifacts.length}`);
    writeWindBuildManifest(
      artifacts[0].executable,
      manifestPath,
      ci ? "ci-direct" : "local-heavy-guard",
    ).catch((error) => {
      console.error(error);
      process.exitCode = 1;
    });
  } else
    main().catch(async (error) => {
      console.error(error);
      try {
        await infrastructureReport(error, process.argv.slice(2));
      } catch (reportError) {
        console.error("Unable to write infrastructure report:", reportError);
      }
      process.exitCode = 1;
    });
}
