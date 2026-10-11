#!/usr/bin/env node
// Identity-normalized gzip for the real island-size measurement (#4131).
//
// The islands entry embeds the 16-hex `zudo_react_build` token: the compiler
// emits it twice per bundle in `__zfb_register` (`identity: { component,
// build }` and the mount closure's identity const, see
// crates/zfb-islands/src/esbuild.rs) and once per island marker in HTML as
// `data-zfb-build` (packages/zfb/src/zudo-react/render-html.ts). The token
// is a source digest over the consumer project including its linked
// `@takazudo/zfb-runtime` sources (crates/zfb-build/src/bundler.rs,
// `zudo_react_build_token_with_snapshots`), so every runtime source edit and
// every version bump moves it. Its fixed length leaves raw totals untouched
// while per-file gzip drifts by a few bytes, which made the exact gzip
// ceiling a token detector rather than a code-growth detector.
//
// This layer is additive: it never edits measure-real.mjs or measure.mjs
// (both are hash-pinned by the Darwin contract) and never changes emitted
// bytes. It reads a saved measurement directory, re-derives every number from
// the saved pass trees, replaces only the validated compiler-owned sites with
// a same-length placeholder, and writes a versioned sidecar next to
// measurement.json. Actual gzip stays in the sidecar as transfer-size evidence.
import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

import {
  chunkRole,
  emittedJavascript,
  initialJavascript,
  readDirectoryFiles,
} from "./chunk-inventory.mjs";

const scriptPath = fileURLToPath(import.meta.url);

export const IDENTITY_NORMALIZED_METRIC = "gzip-identity-normalized";
export const IDENTITY_NORMALIZATION_VERSION = 1;
export const IDENTITY_PLACEHOLDER = "0000000000000000";
export const NORMALIZED_SIDECAR = "measurement.normalized.json";
export const NORMALIZER_FILE = "normalize-identity.mjs";
// Compiler-owned sites per chunk role, measured on the v4.3.1 Linux x64
// artifact (health run 38075998971): every islands entry carries exactly two
// `build:"<token>"` sites and no other 16-hex run; shared and lazy chunks
// carry none. A different count is a product change that needs a new
// normalization version, not a silent accept.
export const IDENTITY_SITES_BY_ROLE = Object.freeze({ entry: 2, shared: 0, lazy: 0 });
export const IDENTITY_NORMALIZATION_RULES =
  "v1: read the zudo_react_build token from every data-zfb-build attribute in the saved " +
  "index.html (all must be one identical 16-character lowercase hex value); in each emitted JS " +
  'file replace only exact build:"<that token>" occurrences in the emitted registration ' +
  'identity:{component,build} and mount-local let identity shapes (outside comments and strings) with build:"' +
  IDENTITY_PLACEHOLDER +
  '" (same length, so raw is unchanged); require exactly 2 sites in the islands entry and 0 in ' +
  "shared/lazy chunks; reject any recognized compiler identity site carrying another token; a dist with no JS " +
  "must carry no token; gzipNormalized is node:zlib gzipSync level=9 mtime=0 of the normalized " +
  "bytes, summed per entry/shared/lazy role exactly like the actual gzip.";

const tokenPattern = /^[0-9a-f]{16}$/;
const htmlTokenPattern = /data-zfb-build="([^"]*)"/g;
// Match the two object shapes emitted by __zfb_register, rather than an
// arbitrary object with a property named build. The first is the registration
// identity; the second is the mount closure's local identity.
const buildSitePatterns = [
  /identity:\{component:[\w$]+,build:"([0-9a-f]{16})"/g,
  /let [\w$]+=\{component:[\w$]+,build:"([0-9a-f]{16})"/g,
];

// A candidate inside a comment or string is not executable compiler output.
// Preserve indices so the candidate patterns can still inspect the source.
function nonCodeMask(text) {
  const mask = text.split("");
  let state = "code";
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    const next = text[index + 1];
    if (state === "code") {
      if (char === '"' || char === "'" || char === "`") state = char;
      else if (char === "/" && next === "/") state = "line";
      else if (char === "/" && next === "*") state = "block";
      else continue;
      mask[index] = " ";
      continue;
    }
    mask[index] = " ";
    if (state === "line" && char === "\n") state = "code";
    else if (state === "block" && char === "*" && next === "/") {
      mask[++index] = " ";
      state = "code";
    } else if ((state === '"' || state === "'" || state === "`") && char === "\\") {
      mask[++index] = " ";
    } else if (state === char && (char === '"' || char === "'" || char === "`")) {
      state = "code";
    }
  }
  return mask.join("");
}

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export function normalizerSha256() {
  // The inventory helper decides which bytes and chunk roles enter the metric.
  // Pin it together with this file so either implementation change invalidates
  // the reviewed contract and every previously generated sidecar.
  return createHash("sha256")
    .update("normalize-identity.mjs\0")
    .update(readFileSync(scriptPath))
    .update("\0chunk-inventory.mjs\0")
    .update(readFileSync(resolve(dirname(scriptPath), "chunk-inventory.mjs")))
    .digest("hex");
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

// Returns the single build token declared by the HTML, or null when the page
// declares none. Any malformed or disagreeing attribute is fatal.
export function buildTokenFromHtml(htmlText, label = "index.html") {
  const tokens = [...htmlText.matchAll(htmlTokenPattern)].map((match) => match[1]);
  const attributes = [...htmlText.matchAll(/\bdata-zfb-build\b/gi)];
  if (attributes.length !== tokens.length) {
    throw new Error(
      label + ': malformed data-zfb-build attribute; expected data-zfb-build="<16 lowercase hex>"',
    );
  }
  if (tokens.length === 0) return null;
  for (const token of tokens) {
    if (!tokenPattern.test(token)) {
      throw new Error(
        label +
          ": data-zfb-build must be 16 lowercase hex characters, found " +
          JSON.stringify(token),
      );
    }
  }
  const unique = new Set(tokens);
  if (unique.size !== 1) {
    throw new Error(
      label + ": data-zfb-build attributes disagree: " + [...unique].sort().join(", "),
    );
  }
  return tokens[0];
}

// Replaces the exact `build:"<token>"` sites with the placeholder and reports
// how many were replaced. Works on latin1 so every byte round-trips unchanged.
export function normalizeIdentitySites(bytes, token, expectedSites, label) {
  const text = bytes.toString("latin1");
  const codeMask = nonCodeMask(text);
  const sites = buildSitePatterns
    .flatMap((pattern) => [...text.matchAll(pattern)])
    .filter((match) => codeMask[match.index] !== " ");
  const foreign = sites.map((match) => match[1]).filter((found) => found !== token);
  if (foreign.length > 0) {
    throw new Error(
      label +
        ": build identity site carries " +
        [...new Set(foreign)].sort().join(", ") +
        " but index.html declares " +
        (token ?? "no token"),
    );
  }
  if (sites.length !== expectedSites) {
    throw new Error(
      label + ": expected " + expectedSites + " build identity site(s), found " + sites.length,
    );
  }
  if (sites.length === 0) return { bytes, sites: 0 };
  const normalized = text.split("");
  for (const site of sites) {
    const tokenIndex = site.index + site[0].length - token.length - 1;
    normalized.splice(tokenIndex, token.length, ...IDENTITY_PLACEHOLDER);
  }
  const output = Buffer.from(normalized.join(""), "latin1");
  if (output.length !== bytes.length) {
    throw new Error(label + ": normalization changed the raw length");
  }
  return { bytes: output, sites: sites.length };
}

function gzipLength(bytes) {
  return gzipSync(bytes, { level: 9, mtime: 0 }).length;
}

function sum(chunks) {
  return {
    raw: chunks.reduce((total, chunk) => total + chunk.raw, 0),
    gzip: chunks.reduce((total, chunk) => total + chunk.gzip, 0),
    gzipNormalized: chunks.reduce((total, chunk) => total + chunk.gzipNormalized, 0),
  };
}

// Recomputes one case from the bytes of a saved pass tree.
export function normalizeFixtureFiles(files, label) {
  const html = files["index.html"];
  if (!html) throw new Error(label + ": dist is missing index.html");
  const token = buildTokenFromHtml(html.toString("utf8"), label + " index.html");
  const { jsPaths, entryPaths } = emittedJavascript(files);
  if (jsPaths.length === 0 && token !== null) {
    throw new Error(label + ": index.html declares a build token but no JavaScript was emitted");
  }
  if (jsPaths.length > 0 && token === null) {
    throw new Error(label + ": JavaScript was emitted but index.html declares no build token");
  }
  if (jsPaths.length > 0 && entryPaths.length !== 1) {
    throw new Error(label + ": expected exactly one islands entry, found " + entryPaths.length);
  }
  const { initial, errors } = initialJavascript(files, jsPaths, entryPaths, label);
  if (errors.length > 0) throw new Error(errors.join("; "));

  const chunks = jsPaths.map((file) => {
    const bytes = files[file];
    const role = chunkRole(file, entryPaths, initial);
    const normalized = normalizeIdentitySites(
      bytes,
      token,
      IDENTITY_SITES_BY_ROLE[role],
      label + " " + file,
    );
    return {
      file,
      phase: initial.has(file) ? "initial" : "later",
      role,
      raw: bytes.length,
      gzip: gzipLength(bytes),
      gzipNormalized: gzipLength(normalized.bytes),
      identitySites: normalized.sites,
      sha256: sha256(bytes),
      sha256Normalized: sha256(normalized.bytes),
    };
  });
  return {
    buildToken: token,
    entryPaths,
    chunks,
    initial: sum(chunks.filter((chunk) => chunk.phase === "initial")),
    later: sum(chunks.filter((chunk) => chunk.phase === "later")),
    shipped: sum(chunks),
  };
}

function sameBytes(left, right) {
  const leftNames = Object.keys(left).sort();
  const rightNames = Object.keys(right).sort();
  return (
    JSON.stringify(leftNames) === JSON.stringify(rightNames) &&
    leftNames.every((name) => left[name].equals(right[name]))
  );
}

function assertClaimedResult(claimed, actual, label) {
  if (!isRecord(claimed) || !Array.isArray(claimed.chunks)) {
    throw new Error(label + ": measurement.json has no chunk inventory for this case");
  }
  const claimedFiles = claimed.chunks.map((chunk) => chunk?.file).sort();
  const actualFiles = actual.chunks.map((chunk) => chunk.file);
  if (JSON.stringify(claimedFiles) !== JSON.stringify(actualFiles)) {
    throw new Error(label + ": measurement.json chunk list differs from the saved JS files");
  }
  for (const chunk of actual.chunks) {
    const summary = claimed.chunks.find((item) => item?.file === chunk.file);
    for (const field of ["raw", "gzip", "sha256", "phase", "role"]) {
      if (summary[field] !== chunk[field]) {
        throw new Error(
          label +
            ": measurement.json claims " +
            field +
            " " +
            String(summary[field]) +
            " for " +
            chunk.file +
            " but the saved bytes give " +
            String(chunk[field]),
        );
      }
    }
  }
  for (const key of ["initial", "later", "shipped"]) {
    const size = claimed[key];
    if (!isRecord(size) || size.raw !== actual[key].raw || size.gzip !== actual[key].gzip) {
      throw new Error(
        label +
          ": measurement.json " +
          key +
          " totals " +
          JSON.stringify(size) +
          " disagree with the saved bytes (raw " +
          actual[key].raw +
          ", gzip " +
          actual[key].gzip +
          ")",
      );
    }
  }
}

// `artifacts` maps each case directory to its two saved pass trees
// (`{ passes: { 1: { files }, 2: { files } } }`), the shape check-budget's
// loadRun produces. Everything numeric is re-derived from pass-1 bytes after
// proving pass-2 is byte-identical.
export function normalizeMeasurement({ measurementText, artifacts, normalizerDigest }) {
  const measurement = JSON.parse(measurementText);
  if (!isRecord(measurement.results) || !isRecord(measurement.provenance)) {
    throw new Error("measurement.json is missing results or provenance");
  }
  const caseNames = Object.keys(measurement.results).sort();
  if (JSON.stringify(caseNames) !== JSON.stringify(Object.keys(artifacts).sort())) {
    throw new Error("saved case directories differ from measurement.json results");
  }
  const results = {};
  for (const fixture of caseNames) {
    const label = measurement.provenance.mode + " " + fixture;
    const passes = artifacts[fixture]?.passes;
    if (!isRecord(passes?.["1"]?.files) || !isRecord(passes?.["2"]?.files)) {
      throw new Error(label + ": output is missing a complete two-pass artifact matrix");
    }
    if (!sameBytes(passes["1"].files, passes["2"].files)) {
      throw new Error(label + ": saved repeated dist inventories differ");
    }
    const actual = normalizeFixtureFiles(passes["1"].files, label);
    assertClaimedResult(measurement.results[fixture], actual, label);
    results[fixture] = actual;
  }
  const provenance = measurement.provenance;
  return {
    schemaVersion: 1,
    metric: IDENTITY_NORMALIZED_METRIC,
    normalizationVersion: IDENTITY_NORMALIZATION_VERSION,
    placeholder: IDENTITY_PLACEHOLDER,
    identitySitesByRole: { ...IDENTITY_SITES_BY_ROLE },
    rules: IDENTITY_NORMALIZATION_RULES,
    compression: provenance.compression,
    source: {
      file: "measurement.json",
      sha256: sha256(Buffer.from(measurementText, "utf8")),
      mode: provenance.mode,
      sourceSha: provenance.sourceSha,
      platform: provenance.platform,
      packageVersion: provenance.packageVersion,
      nodeVersion: provenance.nodeVersion,
      zlibVersion: provenance.zlibVersion,
      runnerSha256: provenance.runnerSha256,
    },
    normalizer: {
      file: NORMALIZER_FILE,
      sha256: normalizerDigest ?? normalizerSha256(),
      nodeVersion: process.version,
      zlibVersion: process.versions.zlib,
      platform: { os: process.platform, arch: process.arch },
    },
    results,
  };
}

export function loadMeasurementDirectory(directory) {
  const outDir = resolve(directory);
  const measurementText = readFileSync(resolve(outDir, "measurement.json"), "utf8");
  const artifacts = {};
  for (const entry of readdirSync(outDir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const casePath = resolve(outDir, entry.name);
    const passes = {};
    for (const passDir of readdirSync(casePath, { withFileTypes: true })) {
      if (!passDir.isDirectory()) {
        throw new Error("unexpected case output path: " + entry.name + "/" + passDir.name);
      }
      const number =
        passDir.name === "pass-1" ? "1" : passDir.name === "pass-2" ? "2" : passDir.name;
      passes[number] = { files: readDirectoryFiles(resolve(casePath, passDir.name)) };
    }
    artifacts[entry.name] = { passes };
  }
  return { outDir, measurementText, artifacts };
}

export function normalizeMeasurementDirectory(directory) {
  const { outDir, measurementText, artifacts } = loadMeasurementDirectory(directory);
  return { outDir, sidecar: normalizeMeasurement({ measurementText, artifacts }) };
}

function parseArgs(args) {
  const values = {};
  for (let index = 0; index < args.length; index += 2) {
    const name = args[index];
    const value = args[index + 1];
    if (name !== "--measurement" && name !== "--out") throw new Error("unknown argument " + name);
    if (!value || value.startsWith("--")) throw new Error("missing value for " + name);
    if (Object.hasOwn(values, name)) throw new Error("duplicate argument " + name);
    values[name] = value;
  }
  if (!values["--measurement"]) throw new Error("required --measurement <dir>/measurement.json");
  return values;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const measurementPath = resolve(args["--measurement"]);
  const { outDir, sidecar } = normalizeMeasurementDirectory(dirname(measurementPath));
  const outPath = resolve(args["--out"] ?? resolve(outDir, NORMALIZED_SIDECAR));
  writeFileSync(outPath, JSON.stringify(sidecar, null, 2) + "\n");
  for (const [fixture, result] of Object.entries(sidecar.results)) {
    console.log(
      fixture.padEnd(14) +
        " raw " +
        String(result.shipped.raw).padStart(6) +
        "  gzip " +
        String(result.shipped.gzip).padStart(6) +
        "  gzipNormalized " +
        String(result.shipped.gzipNormalized).padStart(6) +
        "  token " +
        (result.buildToken ?? "-"),
    );
  }
  console.log(outPath);
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  try {
    main();
  } catch (error) {
    console.error("identity normalization rejected: " + (error?.stack ?? String(error)));
    process.exitCode = 1;
  }
}
