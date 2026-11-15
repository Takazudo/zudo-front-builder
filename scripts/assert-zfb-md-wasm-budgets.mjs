#!/usr/bin/env node

// zfb#2454: fail-closed production artifact metrics. The build log is the
// only place the four overwritten cdylib/bindgen outputs remain observable;
// final wasm/glue and the complete dist are measured from files instead of
// trusting a human-readable summary.

import {
  appendFileSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { gzipSync } from "node:zlib";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { tmpdir } from "node:os";
const MANIFEST_PATH = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../crates/zfb-md-wasm/shipped-sizes.json",
);
const MANIFEST_COLUMNS = ["finalWasm", "gzip9", "glue", "glueGzip9"];
const MANIFEST_ARTIFACTS = {
  default: "root",
  "highlight-only": "highlight",
  "render-only": "render",
  "parse-only": "parse",
};
// node:zlib gzipSync(level 9) drifts by single bytes across runner CPU
// features (#2878); wasm-bindgen/wasm-opt outputs stay byte-exact.
export const GZIP_DRIFT_TOLERANCE_BYTES = 64;
const TOLERANT_COLUMNS = ["gzip9", "glueGzip9"];
const REPAIR_INSTRUCTIONS =
  "optional: re-run with --update-manifest to refresh the historical shipped-sizes.json record";

// Warning references only. They are not loaded from shipped-sizes.json and never fail a build.
export const HISTORICAL_GZIP_CEILINGS = {
  root: 1_600_000,
  highlight: 880_000,
  render: 1_125_000,
  parse: 325_000,
  tarball: 3_900_000,
};
export const ARTIFACTS = [
  { label: "default", dir: "wasm", stem: "zfb_md_wasm", ceiling: HISTORICAL_GZIP_CEILINGS.root },
  {
    label: "highlight-only",
    dir: "wasm-highlight",
    stem: "zfb_md_wasm_highlight",
    ceiling: HISTORICAL_GZIP_CEILINGS.highlight,
  },
  {
    label: "render-only",
    dir: "wasm-render",
    stem: "zfb_md_wasm_render",
    ceiling: HISTORICAL_GZIP_CEILINGS.render,
  },
  {
    label: "parse-only",
    dir: "wasm-parse",
    stem: "zfb_md_wasm_parse",
    ceiling: HISTORICAL_GZIP_CEILINGS.parse,
  },
];
export const TARBALL_CEILING = HISTORICAL_GZIP_CEILINGS.tarball;

function usage() {
  throw new Error(
    "usage: assert-zfb-md-wasm-budgets.mjs --dist <dist> [--build-log <log>] [--tarball <tgz>] [--update-manifest [--measured-on-version <v>]]",
  );
}

function parseArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--update-manifest") {
      values["update-manifest"] = true;
      continue;
    }
    if (!arg.startsWith("--") || argv[index + 1] === undefined) usage();
    values[arg.slice(2)] = argv[++index];
  }
  if (!values.dist) usage();
  if (values["measured-on-version"] !== undefined) {
    if (!values["update-manifest"] || values["measured-on-version"].length === 0) usage();
  }
  return values;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function bytesFromLine(section, label) {
  // Summary labels are literal (not regex fragments): in particular the
  // `wasm-opt -O1 (final)` parentheses must remain literal.
  const match = section.match(
    new RegExp(`^${escapeRegExp(label)}:\\s+([0-9,]+) bytes \\(([0-9]+\\.[0-9]{2}) MB\\)$`, "m"),
  );
  if (!match) throw new Error(`build log is missing ${label} metric`);
  const bytes = Number(match[1].replaceAll(",", ""));
  const expectedMegabytes = (bytes / 1024 / 1024).toFixed(2);
  if (match[2] !== expectedMegabytes) {
    throw new Error(
      `${label} metric has inconsistent MB suffix: ${match[2]} (expected ${expectedMegabytes})`,
    );
  }
  return bytes;
}

function parseBuildLog(logPath) {
  return parseBuildLogText(readFileSync(logPath, "utf8"));
}

function parseBuildLogText(log) {
  const sections = new Map();
  for (const artifact of ARTIFACTS) {
    const marker = `-- ${artifact.label} artifact`;
    const start = log.indexOf(marker);
    if (start < 0) throw new Error(`build log is missing ${artifact.label} summary`);
    const next = ARTIFACTS.map(({ label }) =>
      log.indexOf(`-- ${label} artifact`, start + marker.length),
    )
      .filter((offset) => offset >= 0)
      .sort((a, b) => a - b)[0];
    sections.set(artifact.label, log.slice(start, next ?? log.length));
  }
  return sections;
}

function fileBytes(path) {
  const size = statSync(path).size;
  if (!Number.isSafeInteger(size)) throw new Error(`unsafe file size for ${path}`);
  return size;
}

function distBytes(path) {
  const entry = statSync(path);
  if (entry.isFile()) return entry.size;
  if (!entry.isDirectory()) return 0;
  return readdirSync(path).reduce((total, child) => total + distBytes(resolve(path, child)), 0);
}

function fmt(value) {
  return `${value.toLocaleString("en-US")} bytes`;
}

function measuredValues({ wasm, wasmGzip, glue, glueGzip }) {
  return { finalWasm: wasm, gzip9: wasmGzip, glue, glueGzip9: glueGzip };
}

// Findings never throw mid-loop: main() collects every mismatch/breach across
// all four artifacts first, so a CI run reports them all in one round trip
// instead of one per rerun (zfb#2879). gzip9/glueGzip9 get a warning-only
// band for compressor drift (#2878); every other column stays byte-exact.
export function compareManifest(
  measuredByArtifact,
  documented,
  { tolerance = GZIP_DRIFT_TOLERANCE_BYTES } = {},
) {
  const findings = [];
  for (const artifact of ARTIFACTS) {
    const manifestArtifact = MANIFEST_ARTIFACTS[artifact.label];
    const measured = measuredByArtifact[artifact.label];
    const documentedValues = documented[manifestArtifact];
    for (const column of MANIFEST_COLUMNS) {
      const delta = measured[column] - documentedValues[column];
      if (delta === 0) continue;
      const base = {
        artifact: artifact.label,
        column,
        measured: measured[column],
        documented: documentedValues[column],
        delta,
      };
      if (TOLERANT_COLUMNS.includes(column) && Math.abs(delta) <= tolerance) {
        findings.push({
          ...base,
          code: "gzip-drift-within-tolerance",
          severity: "warning",
          tolerance,
          message:
            `${artifact.label} ${column} (measured=${measured[column]}, ` +
            `documented=${documentedValues[column]}, delta=${delta}) is within the ` +
            `${tolerance}-byte compressor-drift tolerance; realigning with ` +
            `--update-manifest is only expected alongside a real artifact change`,
        });
        continue;
      }
      findings.push({
        ...base,
        code: "manifest-mismatch",
        severity: "warning",
        message:
          `${artifact.label} ${column} mismatch: measured=${measured[column]}, ` +
          `documented=${documentedValues[column]}`,
      });
    }
  }
  return findings;
}

export function checkCeilings(measuredByArtifact) {
  const findings = [];
  for (const artifact of ARTIFACTS) {
    const measured = measuredByArtifact[artifact.label];
    if (measured.gzip9 > artifact.ceiling) {
      findings.push({
        code: "ceiling-exceeded",
        severity: "warning",
        artifact: artifact.label,
        measured: measured.gzip9,
        ceiling: artifact.ceiling,
        message: `${artifact.label} gzip-9 size ${measured.gzip9} exceeds ceiling ${artifact.ceiling}`,
      });
    }
  }
  return findings;
}

export function formatFindings(findings) {
  const lines = findings.map((finding) => finding.message);
  if (findings.some((finding) => finding.code === "manifest-mismatch")) {
    lines.push(REPAIR_INSTRUCTIONS);
  }
  return lines.join("\n");
}

function writeMeasuredManifest(manifestPath, measured, measuredOnVersion) {
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  for (const [artifactLabel, manifestArtifact] of Object.entries(MANIFEST_ARTIFACTS)) {
    for (const column of MANIFEST_COLUMNS) {
      manifest.measured[manifestArtifact][column] = measured[artifactLabel][column];
    }
  }
  if (measuredOnVersion !== undefined) manifest.measuredOnVersion = measuredOnVersion;
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
}

function selfTest() {
  // Fixture mirrors build.mjs's real #2451 summary labels, including commas
  // and literal parentheses in the wasm-opt line.
  const entries = {
    default: ".",
    "highlight-only": "./highlight",
    "render-only": "./render",
    "parse-only": "./parse",
  };
  const fixtureMetrics = {
    default: [3_677_902, 3_448_820, 14_881, 4_151, 3_337_089, 1_491_998],
    "highlight-only": [1_592_558, 1_517_880, 8_758, 2_637, 1_484_531, 766_894],
    "render-only": [2_296_170, 2_183_410, 8_655, 2_601, 2_124_866, 1_033_089],
    "parse-only": [720_146, 670_949, 11_159, 3_797, 653_263, 290_332],
  };
  const fixtureLabels = [
    "raw cdylib",
    "wasm-bindgen binary",
    "generated glue",
    "generated glue gzip -9",
    "wasm-opt -O1 (final)",
    "gzip -9 (final)",
  ];
  const fixture = ARTIFACTS.map(({ label }) => {
    const lines = fixtureMetrics[label].map(
      (bytes, index) =>
        `${fixtureLabels[index]}: ${bytes.toLocaleString("en-US")} bytes (${(bytes / 1024 / 1024).toFixed(2)} MB)`,
    );
    return `-- ${label} artifact (\`${entries[label]}\` entry) --\n${lines.join("\n")}\n`;
  }).join("");
  const sections = parseBuildLogText(fixture);
  for (const artifact of ARTIFACTS) {
    const section = sections.get(artifact.label);
    if (bytesFromLine(section, "raw cdylib") !== fixtureMetrics[artifact.label][0])
      throw new Error("cdylib fixture parse failed");
    if (bytesFromLine(section, "wasm-bindgen binary") !== fixtureMetrics[artifact.label][1])
      throw new Error("bindgen fixture parse failed");
    if (bytesFromLine(section, "wasm-opt -O1 (final)") !== fixtureMetrics[artifact.label][4])
      throw new Error("wasm-opt fixture parse failed");
  }

  const documented = JSON.parse(readFileSync(MANIFEST_PATH, "utf8")).measured;
  const mismatchedMeasuredByArtifact = {
    default: { ...documented.root, finalWasm: documented.root.finalWasm + 1 },
    "highlight-only": documented.highlight,
    "render-only": documented.render,
    "parse-only": documented.parse,
  };
  const mismatchFindings = compareManifest(mismatchedMeasuredByArtifact, documented);
  if (mismatchFindings.length !== 1 || !mismatchFindings[0].message.includes("default finalWasm")) {
    throw new Error("manifest mismatch fixture did not name artifact and column");
  }

  const fixtureDirectory = mkdtempSync(join(tmpdir(), "zfb-md-wasm-budget-self-test-"));
  const fixtureManifestPath = join(fixtureDirectory, "shipped-sizes.json");
  const recorded = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"));
  const fixtureManifest = {
    measuredOnVersion: recorded.measuredOnVersion,
    measured: structuredClone(recorded.measured),
    ceilings: structuredClone(recorded.ceilings),
  };
  const ceilingsBefore = JSON.stringify(fixtureManifest.ceilings);
  const updatedMeasured = {
    default: {
      ...fixtureManifest.measured.root,
      finalWasm: fixtureManifest.measured.root.finalWasm + 2,
    },
    "highlight-only": fixtureManifest.measured.highlight,
    "render-only": fixtureManifest.measured.render,
    "parse-only": fixtureManifest.measured.parse,
  };
  writeFileSync(fixtureManifestPath, `${JSON.stringify(fixtureManifest, null, 2)}\n`);
  writeMeasuredManifest(fixtureManifestPath, updatedMeasured, "2.9.0");
  const updatedManifest = JSON.parse(readFileSync(fixtureManifestPath, "utf8"));
  if (JSON.stringify(updatedManifest.ceilings) !== ceilingsBefore) {
    throw new Error("--update-manifest fixture changed ceilings");
  }
  if (updatedManifest.measured.root.finalWasm !== updatedMeasured.default.finalWasm) {
    throw new Error("--update-manifest fixture did not write measured values");
  }
  if (updatedManifest.measuredOnVersion !== "2.9.0") {
    throw new Error("--update-manifest fixture did not update measuredOnVersion");
  }
  rmSync(fixtureDirectory, { recursive: true, force: true });

  console.log("OK: build summary metric parser self-test");
}

function logDisagreements(artifact, section, wasm, glue) {
  const warnings = [];
  try {
    const loggedFinal = bytesFromLine(section, "wasm-opt -O1 (final)");
    const loggedGlue = bytesFromLine(section, "generated glue");
    if (loggedFinal !== wasm) {
      warnings.push(
        `${artifact.label} final wasm differs from the human build log: log=${loggedFinal}, file=${wasm} (file bytes are the measurement)`,
      );
    }
    if (loggedGlue !== glue) {
      warnings.push(
        `${artifact.label} glue differs from the human build log: log=${loggedGlue}, file=${glue} (file bytes are the measurement)`,
      );
    }
    return {
      warnings,
      cdylib: bytesFromLine(section, "raw cdylib"),
      bindgen: bytesFromLine(section, "wasm-bindgen binary"),
    };
  } catch (error) {
    warnings.push(`${artifact.label} build log was not used as a release check: ${error.message}`);
    return { warnings, cdylib: null, bindgen: null };
  }
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const dist = resolve(args.dist);
  const measuredByArtifact = {};
  const logWarnings = [];
  let sections = null;
  if (args["build-log"]) {
    try {
      sections = parseBuildLog(args["build-log"]);
    } catch (error) {
      logWarnings.push(`build log was not used as a release check: ${error.message}`);
    }
  }

  if (!statSync(dist).isDirectory()) throw new Error(`dist directory is missing: ${dist}`);

  for (const artifact of ARTIFACTS) {
    const wasmPath = resolve(dist, artifact.dir, `${artifact.stem}_bg.wasm`);
    const gluePath = resolve(dist, artifact.dir, `${artifact.stem}_glue.zfb-resource.mjs`);
    const wasm = fileBytes(wasmPath);
    const wasmGzip = gzipSync(readFileSync(wasmPath), { level: 9 }).length;
    const glue = fileBytes(gluePath);
    const glueGzip = gzipSync(readFileSync(gluePath), { level: 9 }).length;
    const measured = measuredValues({ wasm, wasmGzip, glue, glueGzip });
    let cdylib = null;
    let bindgen = null;
    if (sections) {
      const compared = logDisagreements(artifact, sections.get(artifact.label), wasm, glue);
      logWarnings.push(...compared.warnings);
      cdylib = compared.cdylib;
      bindgen = compared.bindgen;
    }

    console.log(
      `${artifact.label}: cdylib=${cdylib === null ? "n/a" : fmt(cdylib)} bindgen=${bindgen === null ? "n/a" : fmt(bindgen)} ` +
        `final-wasm=${fmt(wasm)} gzip-9=${fmt(wasmGzip)} ` +
        `glue=${fmt(glue)} glue-gzip-9=${fmt(glueGzip)} ceiling=${fmt(artifact.ceiling)}`,
    );

    measuredByArtifact[artifact.label] = measured;
  }

  let recordedMeasured = null;
  try {
    recordedMeasured = JSON.parse(readFileSync(MANIFEST_PATH, "utf8")).measured;
  } catch {
    recordedMeasured = null;
  }
  const findings = [
    ...logWarnings.map((message) => ({ severity: "warning", message })),
    ...checkCeilings(measuredByArtifact),
    ...(args["update-manifest"] || !recordedMeasured
      ? []
      : compareManifest(measuredByArtifact, recordedMeasured)),
  ];
  if (findings.length > 0) {
    const lines = [
      "MD/WASM size report (informational; historical measurements and old ceilings are not failures):",
      ...findings.map((finding) => finding.message),
    ];
    for (const line of lines) console.log(line);
    if (process.env.GITHUB_STEP_SUMMARY) {
      appendFileSync(process.env.GITHUB_STEP_SUMMARY, lines.join("\n") + "\n");
    }
  }

  const totalDist = distBytes(dist);
  console.log(`complete dist: ${fmt(totalDist)}`);

  if (args.tarball) {
    const tarball = fileBytes(resolve(args.tarball));
    console.log(
      `complete packed tarball: ${fmt(tarball)} historical-ceiling=${fmt(TARBALL_CEILING)}`,
    );
    if (tarball > TARBALL_CEILING) {
      console.log(
        `packed tarball size ${tarball} exceeds historical ceiling ${TARBALL_CEILING} (informational)`,
      );
    }
  } else {
    console.log(`complete packed tarball: not supplied (ceiling=${fmt(TARBALL_CEILING)})`);
  }

  if (args["update-manifest"]) {
    writeMeasuredManifest(MANIFEST_PATH, measuredByArtifact, args["measured-on-version"]);
    console.log(
      `updated ${MANIFEST_PATH} measured section${args["measured-on-version"] ? ` and measuredOnVersion=${args["measured-on-version"]}` : ""}`,
    );
  }
}

const argument = process.argv[1];
if (argument !== undefined && import.meta.url === pathToFileURL(argument).href) {
  if (process.argv.includes("--self-test")) selfTest();
  else main();
}
