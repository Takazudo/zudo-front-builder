#!/usr/bin/env node
// Validate real zfb island-size runs against the reviewed, per-fixture contract.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync, gzipSync } from "node:zlib";

const scriptPath = fileURLToPath(import.meta.url);
const scriptDir = dirname(scriptPath);
const repoRoot = resolve(scriptDir, "../..");
const fixtureNames = [
  "no-island",
  "event-only",
  "scalar-signal",
  "show-for",
  "model",
  "blog-theme",
  "json-api",
  "multi-island",
];
const fixtureFiles = [
  "event-only",
  "scalar-signal",
  "show-for",
  "model",
  "blog-menu",
  "theme-toggle",
  "json-api",
];
const runnerFiles = ["measure-real.mjs", "measure.mjs"];
const contractFilesByPlatform = {
  "darwin-arm64": "decision.json",
  "linux-x64": "decision-linux-x64.json",
};
const expectedToolchain = {
  honoVersion: "4.12.25",
  esbuildVersion: "0.25.12",
  nodeVersion: "v24.14.0",
  zlibVersion: "1.3.1-e00f703",
};
const compression =
  "node:zlib gzipSync level=9 mtime=0 per emitted JS file; sum every unique entry/shared/lazy file";
const shaPattern = /^[0-9a-f]{64}$/;
const packageVersionPattern = /^\d+\.\d+\.\d+(?:-next\.\d+)?$/;
const sourceShaPattern = /^[0-9a-f]{40}$/;
const javascriptPattern = /\.(?:js|mjs|cjs)$/;
const entryPattern = /^assets\/islands(?:-[^/]+)?\.js$/;

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

// A lockstep release changes workspace specifiers without changing the pinned
// dependency graph used for the reviewed size measurement. Rebase only those
// exact specifier values before comparing the current lockfile to its snapshot.
export function pnpmLockHashForContract(lockText, currentVersion, contractVersion) {
  const currentSpecifier = `specifier: workspace:${currentVersion}`;
  const contractSpecifier = `specifier: workspace:${contractVersion}`;
  return sha256(lockText.replaceAll(currentSpecifier, contractSpecifier));
}

function duplicateJsonKeys(text) {
  const duplicates = [];
  let index = 0;
  const skipWhitespace = () => {
    while (/\s/.test(text[index] ?? "")) index += 1;
  };
  const parseString = () => {
    const start = index;
    index += 1;
    while (index < text.length) {
      if (text[index] === "\\") {
        index += 2;
      } else if (text[index] === '"') {
        index += 1;
        return JSON.parse(text.slice(start, index));
      } else {
        index += 1;
      }
    }
    throw new Error("unterminated JSON string");
  };
  const parseValue = (path) => {
    skipWhitespace();
    if (text[index] === "{") {
      index += 1;
      skipWhitespace();
      const seen = new Set();
      if (text[index] === "}") {
        index += 1;
        return;
      }
      while (index < text.length) {
        skipWhitespace();
        const key = parseString();
        if (seen.has(key)) duplicates.push(path + "." + key);
        seen.add(key);
        skipWhitespace();
        if (text[index] !== ":") throw new Error("expected JSON object colon");
        index += 1;
        parseValue(path + "." + key);
        skipWhitespace();
        if (text[index] === "}") {
          index += 1;
          return;
        }
        if (text[index] !== ",") throw new Error("expected JSON object comma");
        index += 1;
      }
      throw new Error("unterminated JSON object");
    }
    if (text[index] === "[") {
      index += 1;
      skipWhitespace();
      if (text[index] === "]") {
        index += 1;
        return;
      }
      let item = 0;
      while (index < text.length) {
        parseValue(path + "[" + item + "]");
        item += 1;
        skipWhitespace();
        if (text[index] === "]") {
          index += 1;
          return;
        }
        if (text[index] !== ",") throw new Error("expected JSON array comma");
        index += 1;
      }
      throw new Error("unterminated JSON array");
    }
    if (text[index] === '"') {
      parseString();
      return;
    }
    while (index < text.length && !/[,\]}]/.test(text[index]) && !/\s/.test(text[index])) {
      index += 1;
    }
  };
  try {
    parseValue("$");
  } catch {
    return [];
  }
  return duplicates;
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function sortedKeys(value) {
  return isRecord(value) ? Object.keys(value).sort() : [];
}

function sameKeys(value, expected) {
  return JSON.stringify(sortedKeys(value)) === JSON.stringify([...expected].sort());
}

function validCount(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function platformId(platform) {
  if (!isRecord(platform) || !sameKeys(platform, ["os", "arch"])) return null;
  if (typeof platform.os !== "string" || typeof platform.arch !== "string") return null;
  return platform.os + "-" + platform.arch;
}

function currentPlatform() {
  return { os: process.platform, arch: process.arch };
}

export function contractPathForPlatform(platform = currentPlatform(), directory = scriptDir) {
  const id = platformId(platform);
  const filename = id ? contractFilesByPlatform[id] : null;
  if (!filename) {
    throw new Error(
      "no reviewed island-size contract for platform " +
        (id ?? JSON.stringify(platform)) +
        "; supported platforms: " +
        Object.keys(contractFilesByPlatform).join(", "),
    );
  }
  return resolve(directory, filename);
}

function safeRelativePath(value) {
  if (typeof value !== "string" || value.length === 0) return false;
  if (value.includes("\\") || value.startsWith("/") || value.includes("\0")) return false;
  const parts = value.split("/");
  return parts.every((part) => part !== "" && part !== "." && part !== "..");
}

function compareInventory(files, reported, label, errors) {
  if (!isRecord(files)) {
    errors.push(label + ": artifact output is missing");
    return null;
  }
  if (!Array.isArray(reported)) {
    errors.push(label + ": measurement.files must be an array");
    return null;
  }

  const actualEntries = Object.entries(files)
    .map(([path, bytes]) => ({ path, bytes: Buffer.from(bytes) }))
    .sort((left, right) => (left.path < right.path ? -1 : left.path > right.path ? 1 : 0));
  const actualPaths = actualEntries.map((entry) => entry.path);
  const reportedPaths = [];
  for (const entry of reported) {
    if (!isRecord(entry) || !safeRelativePath(entry.path) || !shaPattern.test(entry.sha256 ?? "")) {
      errors.push(label + ": invalid measurement.files entry");
      continue;
    }
    reportedPaths.push(entry.path);
  }
  if (new Set(actualPaths).size !== actualPaths.length) {
    errors.push(label + ": duplicate artifact paths");
  }
  if (new Set(reportedPaths).size !== reportedPaths.length) {
    errors.push(label + ": duplicate paths in measurement.files");
  }
  if (JSON.stringify(actualPaths) !== JSON.stringify([...reportedPaths].sort())) {
    errors.push(label + ": saved dist inventory differs from measurement.files");
  }

  for (const entry of actualEntries) {
    if (!safeRelativePath(entry.path)) {
      errors.push(label + ": unsafe artifact path " + entry.path);
      continue;
    }
    const summary = reported.find((item) => item?.path === entry.path);
    const actualHash = sha256(entry.bytes);
    if (!summary) continue;
    if (summary.sha256 !== actualHash) {
      errors.push(label + ": stale SHA-256 for " + entry.path);
    }
  }
  return Object.fromEntries(actualEntries.map(({ path, bytes }) => [path, bytes]));
}

function staticImports(code) {
  const paths = [];
  const patterns = [
    /\bimport\s*(?!\()(?:[^;'"()]*?\bfrom\s*)?["'](\.\.?\/[^"']+\.(?:js|mjs|cjs))["']/g,
    /\bexport\s+[^;'"()]*?\bfrom\s*["'](\.\.?\/[^"']+\.(?:js|mjs|cjs))["']/g,
  ];
  for (const pattern of patterns) {
    for (const match of code.matchAll(pattern)) paths.push(match[1]);
  }
  return paths;
}

function resolveImport(from, specifier) {
  const parts = from.split("/").slice(0, -1).concat(specifier.split("/"));
  const normalized = [];
  for (const part of parts) {
    if (part === "" || part === ".") continue;
    if (part === "..") {
      if (normalized.length === 0) return null;
      normalized.pop();
    } else {
      normalized.push(part);
    }
  }
  return normalized.join("/");
}

function initialJavascript(files, jsPaths, entryPaths, label, errors) {
  const initial = new Set();
  const html = files["index.html"];
  if (!html) {
    errors.push(label + ": dist is missing index.html");
    return initial;
  }

  const emitted = new Set(jsPaths);
  const visit = (path) => {
    if (initial.has(path)) return;
    const bytes = files[path];
    if (!bytes) {
      errors.push(label + ": missing imported chunk " + path);
      return;
    }
    initial.add(path);
    const code = bytes.toString("utf8");
    for (const specifier of staticImports(code)) {
      const resolved = resolveImport(path, specifier);
      if (!resolved || !emitted.has(resolved)) {
        errors.push(
          label +
            ": static import does not resolve to an emitted JS file: " +
            path +
            " -> " +
            specifier,
        );
      } else {
        visit(resolved);
      }
    }
  };

  for (const path of entryPaths) visit(path);
  const htmlScripts = [
    ...html
      .toString("utf8")
      .matchAll(/(?:src|href)=["']([^"']+\.(?:js|mjs|cjs))(?:[?#][^"']*)?["']/g),
  ];
  for (const match of htmlScripts) {
    const rawPath = match[1];
    if (/^(?:[a-z]+:)?\/\//i.test(rawPath)) continue;
    const path = rawPath.replace(/^\//, "").split(/[?#]/, 1)[0];
    if (emitted.has(path)) visit(path);
  }
  return initial;
}

function sumChunks(chunks) {
  return {
    raw: chunks.reduce((total, chunk) => total + chunk.raw, 0),
    gzip: chunks.reduce((total, chunk) => total + chunk.gzip, 0),
  };
}

function equalSize(left, right) {
  return (
    isRecord(left) &&
    isRecord(right) &&
    Number.isSafeInteger(left.raw) &&
    Number.isSafeInteger(left.gzip) &&
    left.raw === right.raw &&
    left.gzip === right.gzip
  );
}

function validateContract(contract, localInputs, runningPlatform, errors) {
  if (!isRecord(contract)) {
    errors.push("contract: expected a JSON object");
    return;
  }
  if (contract.schemaVersion !== 1) errors.push("contract: unsupported schemaVersion");
  if (contract.fixtureVersion !== "v3-islands-1")
    errors.push("contract: unsupported fixtureVersion");
  if (!sourceShaPattern.test(contract.baselineSourceSha ?? "")) {
    errors.push("contract: baselineSourceSha must be a full git SHA");
  }
  const contractPlatformId = platformId(contract.platform);
  const runningPlatformId = platformId(runningPlatform);
  if (!contractPlatformId) {
    errors.push("contract: platform must include exactly os and arch strings");
  } else if (!contractFilesByPlatform[contractPlatformId]) {
    errors.push("contract: unsupported platform " + contractPlatformId);
  }
  if (!runningPlatformId || !contractFilesByPlatform[runningPlatformId]) {
    errors.push(
      "runtime: no reviewed island-size contract for platform " +
        (runningPlatformId ?? JSON.stringify(runningPlatform)),
    );
  }
  if (contractPlatformId && runningPlatformId && contractPlatformId !== runningPlatformId) {
    errors.push(
      "contract: platform " +
        contractPlatformId +
        " does not match running platform " +
        runningPlatformId,
    );
  }
  if (contract.compression !== compression) errors.push("contract: unsupported compression method");
  if (!isRecord(contract.allowance)) errors.push("contract: missing allowance");
  else {
    for (const metric of ["raw", "gzip"]) {
      if (!validCount(contract.allowance[metric]))
        errors.push("contract: invalid " + metric + " allowance");
      else if (contract.allowance[metric] !== 0) {
        errors.push("contract: island-size allowance must remain zero for " + metric);
      }
    }
  }
  if (!isRecord(contract.toolchain)) errors.push("contract: missing toolchain");
  else {
    if (!packageVersionPattern.test(contract.toolchain.packageVersion ?? "")) {
      errors.push("contract: invalid package version");
    }
    for (const [key, expected] of Object.entries(expectedToolchain)) {
      if (contract.toolchain[key] !== expected)
        errors.push("contract: unsupported toolchain " + key);
    }
    for (const key of ["pnpmLockSha256", "cargoLockSha256"]) {
      if (!shaPattern.test(contract.toolchain[key] ?? "")) {
        errors.push("contract: invalid toolchain " + key);
      }
    }
  }
  if (!isRecord(contract.fixtureSha256) || !sameKeys(contract.fixtureSha256, fixtureFiles)) {
    errors.push("contract: fixture SHA map is incomplete or has unknown files");
  } else {
    for (const file of fixtureFiles) {
      if (!shaPattern.test(contract.fixtureSha256[file] ?? "")) {
        errors.push("contract: invalid fixture SHA for " + file);
      }
    }
  }
  if (!isRecord(contract.runnerSha256) || !sameKeys(contract.runnerSha256, runnerFiles)) {
    errors.push("contract: runner SHA map is incomplete or has unknown files");
  } else {
    for (const file of runnerFiles) {
      if (!shaPattern.test(contract.runnerSha256[file] ?? "")) {
        errors.push("contract: invalid runner SHA for " + file);
      }
    }
  }
  for (const mode of ["workspace", "packed"]) {
    const ceilings = contract.ceilings?.[mode];
    if (!isRecord(ceilings) || !sameKeys(ceilings, fixtureNames)) {
      errors.push("contract: " + mode + " ceilings must list every fixture exactly once");
      continue;
    }
    for (const fixture of fixtureNames) {
      const ceiling = ceilings[fixture];
      if (!isRecord(ceiling) || !validCount(ceiling.raw) || !validCount(ceiling.gzip)) {
        errors.push("contract: invalid " + mode + " ceiling for " + fixture);
      } else if (fixture === "no-island" && (ceiling.raw !== 0 || ceiling.gzip !== 0)) {
        errors.push("contract: no-island ceiling must remain exactly zero in " + mode);
      }
    }
  }

  if (!isRecord(localInputs)) {
    errors.push("local inputs: missing repository hash context");
    return;
  }
  if (
    isRecord(contract.fixtureSha256) &&
    !sameHashMaps(contract.fixtureSha256, localInputs.fixtureSha256)
  ) {
    errors.push("local fixtures: SHA-256 values differ from the reviewed contract");
  }
  if (
    isRecord(contract.runnerSha256) &&
    !sameHashMaps(contract.runnerSha256, localInputs.runnerSha256)
  ) {
    errors.push("local measurement runners: SHA-256 values differ from the reviewed contract");
  }
  if (contract.toolchain?.pnpmLockSha256 !== localInputs.pnpmLockSha256) {
    errors.push("local pnpm-lock.yaml: SHA-256 differs from the reviewed contract");
  }
  if (contract.toolchain?.cargoLockSha256 !== localInputs.cargoLockSha256) {
    errors.push("local Cargo.lock: SHA-256 differs from the reviewed contract");
  }
}

function sameHashMaps(expected, actual) {
  return (
    isRecord(expected) &&
    isRecord(actual) &&
    sameKeys(actual, Object.keys(expected)) &&
    Object.keys(expected).every((key) => expected[key] === actual[key])
  );
}

function validateProvenance(
  run,
  mode,
  contract,
  expectedSourceSha,
  runningPlatform,
  localInputs,
  errors,
) {
  const provenance = run.measurement?.provenance;
  const label = mode + " provenance";
  const packageVersion = localInputs.packageVersion ?? contract.toolchain.packageVersion;
  if (!isRecord(provenance)) {
    errors.push(label + ": missing provenance");
    return null;
  }
  if (provenance.mode !== mode) errors.push(label + ": wrong measurement mode");
  if (!platformId(provenance.platform)) {
    errors.push(label + ": missing or invalid platform provenance");
  } else {
    const measuredPlatformId = platformId(provenance.platform);
    const contractPlatformId = platformId(contract.platform);
    const runningPlatformId = platformId(runningPlatform);
    if (measuredPlatformId !== contractPlatformId) {
      errors.push(label + ": measured platform differs from the selected contract");
    }
    if (measuredPlatformId !== runningPlatformId) {
      errors.push(label + ": measured platform differs from the running platform");
    }
  }
  if (provenance.sourceSha !== expectedSourceSha) {
    errors.push(
      label +
        ": source SHA " +
        String(provenance.sourceSha) +
        " does not match requested " +
        expectedSourceSha,
    );
  }
  if (provenance.fixtureVersion !== contract.fixtureVersion) {
    errors.push(label + ": fixture version differs from the contract");
  }
  if (!sameHashMaps(contract.fixtureSha256, provenance.fixtureSha256)) {
    errors.push(label + ": fixture hashes differ from the contract");
  }
  if (!sameHashMaps(contract.runnerSha256, provenance.runnerSha256)) {
    errors.push(label + ": runner hashes differ from the contract");
  }
  if (provenance.packageVersion !== packageVersion) {
    errors.push(label + ": unsupported @takazudo/zfb version");
  }
  if (provenance.runtimePackageVersion !== packageVersion) {
    errors.push(label + ": unsupported @takazudo/zfb-runtime version");
  }
  if (provenance.honoPackageVersion !== expectedToolchain.honoVersion) {
    errors.push(label + ": unsupported Hono version");
  }
  if (provenance.esbuildVersion !== expectedToolchain.esbuildVersion) {
    errors.push(label + ": unsupported esbuild version");
  }
  if (provenance.nodeVersion !== expectedToolchain.nodeVersion) {
    errors.push(label + ": unsupported Node version");
  }
  if (provenance.zlibVersion !== expectedToolchain.zlibVersion) {
    errors.push(label + ": unsupported zlib version");
  }
  if (provenance.compression !== contract.compression) {
    errors.push(label + ": compression metadata differs from the contract");
  }
  if (provenance.repeats !== 2) errors.push(label + ": expected two requested passes");
  for (const key of [
    "packageJsonSha256",
    "runtimePackageJsonSha256",
    "honoPackageJsonSha256",
    "zfbBinarySha256",
    "zfbTarballSha256",
    "runtimeTarballSha256",
    "esbuildBinarySha256",
    "pnpmLockSha256",
    "cargoLockSha256",
  ]) {
    if (!shaPattern.test(provenance[key] ?? "")) errors.push(label + ": invalid " + key);
  }
  const actualInputs = run.actualInputs;
  if (!isRecord(actualInputs)) {
    errors.push(label + ": actual package and tarball inputs are unavailable");
  } else {
    for (const key of [
      "packageJsonSha256",
      "runtimePackageJsonSha256",
      "honoPackageJsonSha256",
      "zfbTarballSha256",
      "runtimeTarballSha256",
    ]) {
      if (provenance[key] !== actualInputs[key]) {
        errors.push(label + ": " + key + " differs from the actual package or tarball");
      }
    }
    for (const [name, manifest, expectedName] of [
      ["zfb package", actualInputs.packageManifest, "@takazudo/zfb"],
      ["runtime package", actualInputs.runtimePackageManifest, "@takazudo/zfb-runtime"],
      ["Hono package", actualInputs.honoPackageManifest, "hono"],
    ]) {
      if (manifest?.name !== expectedName) errors.push(label + ": wrong actual " + name);
    }
    if (actualInputs.packageManifest?.version !== packageVersion) {
      errors.push(label + ": actual @takazudo/zfb manifest has an unsupported version");
    }
    if (actualInputs.runtimePackageManifest?.version !== packageVersion) {
      errors.push(label + ": actual @takazudo/zfb-runtime manifest has an unsupported version");
    }
    if (actualInputs.honoPackageManifest?.version !== expectedToolchain.honoVersion) {
      errors.push(label + ": actual Hono manifest has an unsupported version");
    }
    if (
      actualInputs.runtimePackageManifest?.dependencies?.hono !==
      "^" + expectedToolchain.honoVersion
    ) {
      errors.push(label + ": actual runtime manifest does not declare pinned Hono");
    }
    if (
      mode === "packed" &&
      actualInputs.zfbTarballPackageJsonSha256 !== actualInputs.packageJsonSha256
    ) {
      errors.push(label + ": zfb package manifest differs from its packed tarball");
    }
    if (
      mode === "packed" &&
      actualInputs.runtimeTarballPackageJsonSha256 !== actualInputs.runtimePackageJsonSha256
    ) {
      errors.push(label + ": runtime package manifest differs from its packed tarball");
    }
  }
  if (
    provenance.pnpmLockSha256 !==
    (localInputs.pnpmLockActualSha256 ?? contract.toolchain?.pnpmLockSha256)
  ) {
    errors.push(label + ": pnpm lockfile differs from the current checkout");
  }
  if (provenance.cargoLockSha256 !== contract.toolchain?.cargoLockSha256) {
    errors.push(label + ": Cargo lockfile differs from the contract");
  }
  for (const key of [
    "packagePath",
    "runtimePackagePath",
    "honoPackagePath",
    "zfbTarballPath",
    "runtimeTarballPath",
  ]) {
    if (typeof provenance[key] !== "string" || provenance[key].length === 0) {
      errors.push(label + ": missing " + key);
    }
  }
  return provenance;
}

function validateOneResult(mode, fixture, result, artifactCase, contract, errors) {
  const label = mode + " " + fixture;
  if (!isRecord(result)) {
    errors.push(label + ": missing fixture result");
    return null;
  }
  if (!isRecord(artifactCase)) {
    errors.push(label + ": missing saved artifact output paths");
    return null;
  }
  if (
    !Array.isArray(artifactCase.passDirectories) ||
    JSON.stringify([...artifactCase.passDirectories].sort()) !==
      JSON.stringify(["pass-1", "pass-2"])
  ) {
    errors.push(label + ": output must contain pass-1 and pass-2 only");
  }
  if (!isRecord(artifactCase.passes) || !sameKeys(artifactCase.passes, ["1", "2"])) {
    errors.push(label + ": output is missing a complete two-pass artifact matrix");
    return null;
  }
  if (!Array.isArray(result.files) || !Array.isArray(result.chunks)) {
    errors.push(label + ": missing dist file or JS chunk inventory");
    return null;
  }

  const filesByPass = {};
  for (const passNumber of ["1", "2"]) {
    const pass = artifactCase.passes[passNumber];
    const passLabel = label + " pass-" + passNumber;
    if (
      !isRecord(pass) ||
      !Array.isArray(pass.entries) ||
      JSON.stringify([...pass.entries].sort()) !== JSON.stringify([])
    ) {
      errors.push(passLabel + ": unexpected pass output paths");
    }
    filesByPass[passNumber] = compareInventory(pass?.files, result.files, passLabel, errors);
  }
  const firstFiles = filesByPass["1"];
  const secondFiles = filesByPass["2"];
  if (!firstFiles || !secondFiles) return null;
  const firstNames = Object.keys(firstFiles).sort();
  const secondNames = Object.keys(secondFiles).sort();
  if (
    JSON.stringify(firstNames) !== JSON.stringify(secondNames) ||
    firstNames.some((name) => !firstFiles[name].equals(secondFiles[name]))
  ) {
    errors.push(label + ": saved repeated dist inventories differ");
  }

  const inventory = firstNames.map((path) => ({ path, sha256: sha256(firstFiles[path]) }));
  const repeat = result.repeatVerification;
  if (!isRecord(repeat) || repeat.passes !== 2 || repeat.inventoriesIdentical !== true) {
    errors.push(label + ": missing verified two-pass metadata");
  } else if (repeat.inventorySha256 !== sha256(Buffer.from(JSON.stringify(inventory)))) {
    errors.push(label + ": repeat inventory hash is stale");
  }

  const jsPaths = firstNames.filter((path) => javascriptPattern.test(path));
  const actualEntries = jsPaths
    .filter((path) => entryPattern.test(path) && !path.startsWith("assets/islands-chunk-"))
    .sort();
  if (fixture === "no-island" && jsPaths.length !== 0) {
    errors.push(label + ": no-island must emit no JavaScript");
  }
  if (fixture !== "no-island" && actualEntries.length !== 1) {
    errors.push(label + ": expected exactly one islands entry, found " + actualEntries.length);
  }
  const reportedEntries = Array.isArray(result.entryPaths) ? [...result.entryPaths].sort() : [];
  if (JSON.stringify(reportedEntries) !== JSON.stringify(actualEntries)) {
    errors.push(label + ": entryPaths do not match emitted entry files");
  }
  const html = firstFiles["index.html"]?.toString("utf8") ?? "";
  for (const entry of actualEntries) {
    if (!html.includes("/" + entry))
      errors.push(label + ": entry is not referenced by index.html: " + entry);
  }

  const initial = initialJavascript(firstFiles, jsPaths, actualEntries, label, errors);
  const actualChunks = jsPaths.map((file) => {
    const bytes = firstFiles[file];
    return {
      file,
      phase: initial.has(file) ? "initial" : "later",
      role: actualEntries.includes(file) ? "entry" : initial.has(file) ? "shared" : "lazy",
      raw: bytes.length,
      gzip: gzipSync(bytes, { level: 9, mtime: 0 }).length,
      sha256: sha256(bytes),
    };
  });
  const reportedChunks = result.chunks
    .filter((chunk) => isRecord(chunk) && typeof chunk.file === "string")
    .map((chunk) => chunk.file);
  if (new Set(reportedChunks).size !== reportedChunks.length) {
    errors.push(label + ": duplicate JS chunk metadata");
  }
  if (JSON.stringify([...reportedChunks].sort()) !== JSON.stringify(jsPaths)) {
    errors.push(label + ": emitted JS files differ from chunk metadata");
  }
  for (const actual of actualChunks) {
    const summary = result.chunks.find((chunk) => chunk?.file === actual.file);
    if (!summary) continue;
    for (const field of ["raw", "gzip", "sha256", "phase", "role"]) {
      if (summary[field] !== actual[field]) {
        errors.push(label + ": stale " + field + " metadata for " + actual.file);
      }
    }
  }

  const actualInitial = sumChunks(actualChunks.filter((chunk) => chunk.phase === "initial"));
  const actualLater = sumChunks(actualChunks.filter((chunk) => chunk.phase === "later"));
  const actualShipped = sumChunks(actualChunks);
  for (const [key, actual] of [
    ["initial", actualInitial],
    ["later", actualLater],
    ["shipped", actualShipped],
  ]) {
    if (!equalSize(result[key], actual)) {
      errors.push(label + ": " + key + " totals do not match the emitted JS files");
    }
  }

  const ceiling = contract.ceilings?.[mode]?.[fixture];
  if (isRecord(ceiling) && isRecord(contract.allowance)) {
    const maximum = {
      raw: ceiling.raw + (contract.allowance.raw ?? 0),
      gzip: ceiling.gzip + (contract.allowance.gzip ?? 0),
    };
    const rawDelta = actualShipped.raw - ceiling.raw;
    const gzipDelta = actualShipped.gzip - ceiling.gzip;
    if (actualShipped.raw > maximum.raw || actualShipped.gzip > maximum.gzip) {
      errors.push(
        label +
          " " +
          expectedToolchain.nodeVersion +
          ": baseline raw/gzip " +
          ceiling.raw +
          "/" +
          ceiling.gzip +
          "; actual " +
          actualShipped.raw +
          "/" +
          actualShipped.gzip +
          "; delta " +
          signed(rawDelta) +
          "/" +
          signed(gzipDelta) +
          "; ceiling " +
          maximum.raw +
          "/" +
          maximum.gzip,
      );
    }
  }
  return actualShipped;
}

function signed(value) {
  return value > 0 ? "+" + value : String(value);
}

function compareMatchingProvenance(workspace, packed, errors) {
  if (!workspace || !packed) return;
  const fields = [
    "sourceSha",
    "fixtureVersion",
    "packageVersion",
    "runtimePackageVersion",
    "honoPackageVersion",
    "honoPackageJsonSha256",
    "zfbBinarySha256",
    "zfbTarballSha256",
    "runtimeTarballSha256",
    "esbuildVersion",
    "esbuildBinarySha256",
    "nodeVersion",
    "zlibVersion",
    "compression",
    "repeats",
    "pnpmLockSha256",
    "cargoLockSha256",
  ];
  for (const field of fields) {
    if (workspace[field] !== packed[field]) {
      errors.push("workspace and packed runs have mismatched " + field);
    }
  }
  for (const field of ["fixtureSha256", "runnerSha256"]) {
    if (!sameHashMaps(workspace[field], packed[field])) {
      errors.push("workspace and packed runs have mismatched " + field);
    }
  }
  if (
    workspace.actualInputs?.zfbTarballPackageJsonSha256 !== packed.actualInputs?.packageJsonSha256
  ) {
    errors.push("packed zfb package manifest does not match the workspace run's tarball");
  }
  if (
    workspace.actualInputs?.runtimeTarballPackageJsonSha256 !==
    packed.actualInputs?.runtimePackageJsonSha256
  ) {
    errors.push("packed runtime manifest does not match the workspace run's tarball");
  }
}

function validateRun(run, mode, contract, expectedSourceSha, runningPlatform, localInputs, errors) {
  if (!isRecord(run) || !isRecord(run.measurement)) {
    errors.push(mode + ": missing measurement.json");
    return {};
  }
  const measurement = run.measurement;
  for (const path of run.duplicateKeys ?? []) {
    errors.push(mode + ": duplicate JSON object key " + path);
  }
  const provenance = validateProvenance(
    run,
    mode,
    contract,
    expectedSourceSha,
    runningPlatform,
    localInputs,
    errors,
  );
  const expectedRootEntries = [
    "measurement.json",
    "report.md",
    ...sortedKeys(run.artifacts),
  ].sort();
  if (
    !Array.isArray(run.rootEntries) ||
    JSON.stringify([...run.rootEntries].sort()) !== JSON.stringify(expectedRootEntries)
  ) {
    errors.push(mode + ": measurement output directory has missing or unexpected paths");
  }
  const results = measurement.results;
  if (!isRecord(results) || !sameKeys(results, fixtureNames)) {
    errors.push(mode + ": measurement must contain all eight fixture results exactly once");
  }
  if (!isRecord(run.artifacts) || !sameKeys(run.artifacts, fixtureNames)) {
    errors.push(mode + ": saved outputs must contain all eight fixture directories exactly once");
  }
  for (const fixture of fixtureNames) {
    validateOneResult(
      mode,
      fixture,
      results?.[fixture],
      run.artifacts?.[fixture],
      contract,
      errors,
    );
  }
  return provenance;
}

export function validateIslandSizeBudget({
  workspace,
  packed,
  contract,
  expectedSourceSha,
  localInputs,
  platform = currentPlatform(),
}) {
  const errors = [];
  validateContract(contract, localInputs, platform, errors);
  if (!isRecord(contract)) return { errors, passed: false };
  if (!sourceShaPattern.test(expectedSourceSha ?? "")) {
    errors.push("requested source SHA must be a full git SHA");
  }
  const workspaceProvenance = validateRun(
    workspace,
    "workspace",
    contract,
    expectedSourceSha,
    platform,
    localInputs,
    errors,
  );
  const packedProvenance = validateRun(
    packed,
    "packed",
    contract,
    expectedSourceSha,
    platform,
    localInputs,
    errors,
  );
  compareMatchingProvenance(workspaceProvenance, packedProvenance, errors);
  return { errors, passed: errors.length === 0 };
}

function readDirectoryFiles(directory) {
  const files = {};
  const visit = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const absolute = resolve(current, entry.name);
      const stat = lstatSync(absolute);
      if (stat.isSymbolicLink()) {
        throw new Error("artifact tree contains a symbolic link: " + absolute);
      }
      if (entry.isDirectory()) {
        visit(absolute);
      } else if (entry.isFile()) {
        const path = relative(directory, absolute).split(sep).join("/");
        files[path] = readFileSync(absolute);
      } else {
        throw new Error("artifact tree contains a non-file entry: " + absolute);
      }
    }
  };
  visit(directory);
  return files;
}

function readPackageManifest(packagePath) {
  const bytes = readFileSync(resolve(packagePath, "package.json"));
  return {
    sha256: sha256(bytes),
    manifest: JSON.parse(bytes.toString("utf8")),
  };
}

function tarOctal(header, start, length) {
  const text = header
    .subarray(start, start + length)
    .toString("ascii")
    .replace(/\0.*$/, "")
    .trim();
  const value = text ? Number.parseInt(text, 8) : 0;
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error("invalid tar numeric field");
  }
  return value;
}

function readArchivePackageManifest(tarballPath) {
  const archive = gunzipSync(readFileSync(tarballPath));
  let found = null;
  for (let offset = 0; offset + 512 <= archive.length; ) {
    const header = archive.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) break;
    const storedChecksum = tarOctal(header, 148, 8);
    const checksum = header.reduce(
      (total, byte, index) => total + (index >= 148 && index < 156 ? 0x20 : byte),
      0,
    );
    if (storedChecksum !== checksum) throw new Error("tar header checksum mismatch");
    const name = header.subarray(0, 100).toString("utf8").replace(/\0.*$/, "");
    const prefix = header.subarray(345, 500).toString("utf8").replace(/\0.*$/, "");
    const path = prefix ? prefix + "/" + name : name;
    const size = tarOctal(header, 124, 12);
    const typeFlag = header[156];
    const dataStart = offset + 512;
    const dataEnd = dataStart + size;
    if (dataEnd > archive.length) throw new Error("truncated tar entry");
    if (path === "package/package.json") {
      if (found) throw new Error("duplicate package/package.json tar entry");
      if (typeFlag !== 0 && typeFlag !== 48) {
        throw new Error("package/package.json tar entry is not a regular file");
      }
      found = archive.subarray(dataStart, dataEnd);
    }
    offset = dataStart + Math.ceil(size / 512) * 512;
  }
  if (!found) throw new Error("tarball is missing package/package.json");
  return sha256(found);
}

function loadPackageInputs(provenance) {
  const zfbManifest = readPackageManifest(provenance.packagePath);
  const runtimeManifest = readPackageManifest(provenance.runtimePackagePath);
  const honoManifest = readPackageManifest(provenance.honoPackagePath);
  return {
    packageJsonSha256: zfbManifest.sha256,
    runtimePackageJsonSha256: runtimeManifest.sha256,
    honoPackageJsonSha256: honoManifest.sha256,
    packageManifest: zfbManifest.manifest,
    runtimePackageManifest: runtimeManifest.manifest,
    honoPackageManifest: honoManifest.manifest,
    zfbTarballSha256: sha256(readFileSync(provenance.zfbTarballPath)),
    runtimeTarballSha256: sha256(readFileSync(provenance.runtimeTarballPath)),
    zfbTarballPackageJsonSha256: readArchivePackageManifest(provenance.zfbTarballPath),
    runtimeTarballPackageJsonSha256: readArchivePackageManifest(provenance.runtimeTarballPath),
  };
}

export function loadRun(jsonPath) {
  const absoluteJson = resolve(jsonPath);
  const outDir = dirname(absoluteJson);
  const measurementText = readFileSync(absoluteJson, "utf8");
  const duplicates = duplicateJsonKeys(measurementText);
  const measurement = JSON.parse(measurementText);
  const actualInputs = loadPackageInputs(measurement.provenance ?? {});
  const rootEntries = readdirSync(outDir, { withFileTypes: true }).map((entry) => {
    if (entry.isDirectory()) return entry.name;
    if (!entry.isFile()) throw new Error("unexpected measurement output path: " + entry.name);
    return entry.name;
  });
  const artifacts = {};
  for (const caseDir of rootEntries.filter(
    (entry) => entry !== "measurement.json" && entry !== "report.md",
  )) {
    const casePath = resolve(outDir, caseDir);
    const caseEntries = readdirSync(casePath, { withFileTypes: true }).map((entry) => {
      if (!entry.isDirectory())
        throw new Error("unexpected case output path: " + caseDir + "/" + entry.name);
      return entry.name;
    });
    const passes = {};
    for (const passDir of caseEntries) {
      const number = passDir === "pass-1" ? "1" : passDir === "pass-2" ? "2" : passDir;
      const passPath = resolve(casePath, passDir);
      passes[number] = {
        entries: [],
        files: readDirectoryFiles(passPath),
      };
    }
    artifacts[caseDir] = { passDirectories: caseEntries, passes };
  }
  return { measurement, artifacts, rootEntries, duplicateKeys: duplicates, actualInputs };
}

function currentLocalInputs(contract) {
  const hashFile = (path) => sha256(readFileSync(path));
  const currentVersion = JSON.parse(
    readFileSync(resolve(repoRoot, "packages/zfb/package.json")),
  ).version;
  const pnpmLockText = readFileSync(resolve(repoRoot, "pnpm-lock.yaml"), "utf8");
  return {
    packageVersion: currentVersion,
    fixtureSha256: Object.fromEntries(
      fixtureFiles.map((name) => [name, hashFile(resolve(scriptDir, "fixtures", name + ".tsx"))]),
    ),
    runnerSha256: Object.fromEntries(
      runnerFiles.map((name) => [name, hashFile(resolve(scriptDir, name))]),
    ),
    pnpmLockSha256: pnpmLockHashForContract(
      pnpmLockText,
      currentVersion,
      contract.toolchain.packageVersion,
    ),
    pnpmLockActualSha256: sha256(pnpmLockText),
    cargoLockSha256: hashFile(resolve(repoRoot, "Cargo.lock")),
  };
}

function currentSourceSha() {
  return execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: repoRoot,
    encoding: "utf8",
  }).trim();
}

function parseArgs(args, platform = currentPlatform()) {
  const values = {};
  const allowed = new Set(["--workspace", "--packed", "--contract"]);
  for (let index = 0; index < args.length; index += 2) {
    const name = args[index];
    const value = args[index + 1];
    if (!allowed.has(name)) throw new Error("unknown argument " + name);
    if (!value || value.startsWith("--")) throw new Error("missing value for " + name);
    if (Object.hasOwn(values, name)) throw new Error("duplicate argument " + name);
    values[name] = value;
  }
  if (!values["--workspace"]) throw new Error("required --workspace measurement.json");
  if (!values["--packed"]) throw new Error("required --packed measurement.json");
  return {
    workspacePath: values["--workspace"],
    packedPath: values["--packed"],
    contractPath: values["--contract"] ?? contractPathForPlatform(platform),
    platform,
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2), currentPlatform());
  const contractText = readFileSync(resolve(args.contractPath), "utf8");
  const contractDuplicates = duplicateJsonKeys(contractText);
  if (contractDuplicates.length) {
    throw new Error(
      "contract contains duplicate JSON object key(s): " + contractDuplicates.join(", "),
    );
  }
  const contract = JSON.parse(contractText);
  const result = validateIslandSizeBudget({
    workspace: loadRun(args.workspacePath),
    packed: loadRun(args.packedPath),
    contract,
    expectedSourceSha: currentSourceSha(),
    localInputs: currentLocalInputs(contract),
    platform: args.platform,
  });
  if (!result.passed) {
    console.error("Island size budget rejected:");
    for (const finding of result.errors) console.error("- " + finding);
    process.exitCode = 1;
    return;
  }
  console.log("Island size budget passed for all eight fixtures in workspace and packed modes.");
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  try {
    main();
  } catch (error) {
    console.error("Island size budget rejected: " + (error?.stack ?? String(error)));
    process.exitCode = 1;
  }
}
