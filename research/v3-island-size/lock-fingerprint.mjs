#!/usr/bin/env node
// Dependency-graph fingerprint of pnpm-lock.yaml for the island-size contracts (#4187).
//
// The v2 contracts pin the whole-file `pnpmLockSha256`, so a docs-only or
// root-tooling bump invalidates every native re-pin even though no measured
// byte can move. This helper hashes only the lock content that can produce
// measured bytes: the pnpm environment document in full, plus the project
// document's global resolution settings and the dependency closure of the
// three measured packages. LOCK-FINGERPRINT-POLICY.md (next to this file) is
// the authoritative specification; any change to selection, classification,
// normalization or canonical form is a FINGERPRINT_POLICY_VERSION bump, and
// these bytes are pinned separately by `helperSha256`.
//
// Fail-closed by construction: every record in both documents is schema
// checked (including unreached ones), and any unknown key, unsupported YAML
// node, malformed edge or dangling reference throws a LockFingerprintError
// instead of returning a partial fingerprint.
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, posix, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { isAlias, isMap, isScalar, isSeq, parseAllDocuments } from "yaml";

const scriptPath = fileURLToPath(import.meta.url);
const defaultRepoRoot = resolve(dirname(scriptPath), "../..");

export const FINGERPRINT_POLICY_VERSION = 1;
export const FINGERPRINT_PARSER = "yaml@2.9.0";
export const FINGERPRINT_HELPER_FILE = "lock-fingerprint.mjs";
export const SELECTED_IMPORTERS = Object.freeze([
  "packages/zfb",
  "packages/zfb-runtime",
  "packages/zfb-slugify",
]);
export const LOCKSTEP_PLACEHOLDER = "<lockstep>";
export const LOCKFILE_VERSION = "9.0";
export const BUILD_SCRIPTS = Object.freeze(["build", "prepack", "prepare", "postpack"]);

const ENVIRONMENT_TOP_KEYS = ["lockfileVersion", "importers", "packages", "snapshots"];
const PROJECT_OPTIONAL_GLOBAL_KEYS = [
  "catalogs",
  "overrides",
  "patchedDependencies",
  "pnpmfileChecksum",
  "ignoredOptionalDependencies",
  "packageExtensionsChecksum",
];
const PROJECT_TOP_KEYS = [
  "lockfileVersion",
  "settings",
  ...PROJECT_OPTIONAL_GLOBAL_KEYS,
  "importers",
  "packages",
  "snapshots",
];
const ENVIRONMENT_IMPORTER_KEYS = ["configDependencies", "packageManagerDependencies"];
const PROJECT_IMPORTER_KEYS = ["dependencies", "devDependencies", "optionalDependencies"];
const EDGE_KEYS = ["specifier", "version"];
const PACKAGE_KEYS = [
  "resolution",
  "engines",
  "cpu",
  "os",
  "libc",
  "hasBin",
  "peerDependencies",
  "peerDependenciesMeta",
  "deprecated",
];
const RESOLUTION_KEYS = ["integrity"];
const SNAPSHOT_KEYS = [
  "dependencies",
  "optionalDependencies",
  "optional",
  "transitivePeerDependencies",
];
const SNAPSHOT_EDGE_SECTIONS = ["dependencies", "optionalDependencies"];

export class LockFingerprintError extends Error {
  constructor(code, message) {
    super(`${code}: ${message}`);
    this.name = "LockFingerprintError";
    this.code = code;
  }
}

function fail(code, message) {
  throw new LockFingerprintError(code, message);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function hasOwn(object, key) {
  return Object.prototype.hasOwnProperty.call(object, key);
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function fingerprintHelperSha256(path = scriptPath) {
  return sha256(readFileSync(path));
}

export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (isRecord(value)) {
    const members = Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`);
    return `{${members.join(",")}}`;
  }
  if (
    typeof value === "string" ||
    typeof value === "boolean" ||
    value === null ||
    Number.isSafeInteger(value)
  ) {
    return JSON.stringify(value);
  }
  return fail("CANONICAL_VALUE", `unsupported canonical value of type ${typeof value}`);
}

function nodePlain(node, path) {
  if (node === null || node === undefined) return fail("INVALID_SCALAR", `${path}: empty value`);
  if (isAlias(node)) return fail("UNSUPPORTED_NODE", `${path}: YAML alias`);
  if (node.anchor) return fail("UNSUPPORTED_NODE", `${path}: YAML anchor &${node.anchor}`);
  if (node.tag) return fail("UNSUPPORTED_NODE", `${path}: explicit YAML tag ${node.tag}`);
  if (isMap(node)) {
    const out = Object.create(null);
    for (const pair of node.items) {
      const key = pair.key;
      if (
        !isScalar(key) ||
        isAlias(key) ||
        key.anchor ||
        key.tag ||
        typeof key.value !== "string"
      ) {
        fail("UNSUPPORTED_NODE", `${path}: non-string or non-scalar mapping key`);
      }
      if (key.value === "<<") fail("UNSUPPORTED_NODE", `${path}: YAML merge key`);
      if (hasOwn(out, key.value)) fail("YAML_PARSE", `${path}: duplicate key ${key.value}`);
      out[key.value] = nodePlain(pair.value, `${path}.${key.value}`);
    }
    return out;
  }
  if (isSeq(node)) return node.items.map((item, index) => nodePlain(item, `${path}[${index}]`));
  if (isScalar(node)) {
    if (typeof node.value === "string" || typeof node.value === "boolean") return node.value;
    return fail("INVALID_SCALAR", `${path}: scalar leaves must be strings or booleans`);
  }
  return fail("UNSUPPORTED_NODE", `${path}: unsupported YAML node`);
}

function parseLock(lockText) {
  if (typeof lockText !== "string") fail("INVALID_INPUT", "lockText must be a string");
  const documents = parseAllDocuments(lockText, { uniqueKeys: true });
  for (const [index, document] of Array.from(documents).entries()) {
    const problems = [...document.errors, ...document.warnings];
    if (problems.length > 0) {
      const detail = problems.map((problem) => `${problem.code} ${problem.message.split("\n")[0]}`);
      fail("YAML_PARSE", `document ${index + 1}: ${detail.join("; ")}`);
    }
  }
  if (documents.length !== 2) {
    fail("DOCUMENT_COUNT", `expected exactly 2 YAML documents, found ${documents.length}`);
  }
  return documents.map((document, index) => {
    const label = index === 0 ? "environment" : "project";
    const plain = nodePlain(document.contents, label);
    if (!isRecord(plain)) fail("INVALID_SHAPE", `${label} document is not a mapping`);
    return plain;
  });
}

function checkKeys(record, allowed, path) {
  for (const key of Object.keys(record)) {
    if (!allowed.includes(key)) fail("UNKNOWN_KEY", `${path}: unknown key ${key}`);
  }
}

function expectRecord(value, path) {
  if (!isRecord(value)) fail("INVALID_SHAPE", `${path} must be a mapping`);
  return value;
}

function expectStringArray(value, path) {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    fail("INVALID_SHAPE", `${path} must be a sequence of strings`);
  }
}

function checkPeerSuffix(value, path) {
  const open = value.indexOf("(");
  if (open === 0) fail("MALFORMED_EDGE", `${path}: empty name/version before peer suffix`);
  if (open < 0) {
    if (value.includes(")")) fail("MALFORMED_EDGE", `${path}: unbalanced peer suffix`);
    return;
  }
  let depth = 0;
  for (let index = open; index < value.length; index += 1) {
    const char = value[index];
    if (depth === 0 && char !== "(") fail("MALFORMED_EDGE", `${path}: text outside peer suffix`);
    if (char === "(") depth += 1;
    else if (char === ")") depth -= 1;
    if (depth < 0) fail("MALFORMED_EDGE", `${path}: unbalanced peer suffix`);
  }
  if (depth !== 0) fail("MALFORMED_EDGE", `${path}: unbalanced peer suffix`);
  if (value.slice(0, open).includes(")")) fail("MALFORMED_EDGE", `${path}: unbalanced peer suffix`);
}

function checkSnapshotRef(value, path) {
  if (typeof value !== "string" || value.length === 0) {
    fail("MALFORMED_EDGE", `${path}: edge value must be a non-empty string`);
  }
  if (value.startsWith("link:") || value.startsWith("file:")) {
    fail("MALFORMED_EDGE", `${path}: ${value.slice(0, 5)} is not allowed in a snapshot edge`);
  }
  checkPeerSuffix(value, path);
}

function checkImporterEdges(section, path, allowLink) {
  for (const [name, edge] of Object.entries(expectRecord(section, path))) {
    const edgePath = `${path}.${name}`;
    if (!isRecord(edge)) fail("MALFORMED_EDGE", `${edgePath}: edge must be a mapping`);
    checkKeys(edge, EDGE_KEYS, edgePath);
    for (const key of EDGE_KEYS) {
      if (typeof edge[key] !== "string" || edge[key].length === 0) {
        fail("MALFORMED_EDGE", `${edgePath}: edge needs a non-empty string ${key}`);
      }
    }
    if (edge.version.startsWith("link:")) {
      if (!allowLink) fail("MALFORMED_EDGE", `${edgePath}: link: is not allowed here`);
      if (edge.version === "link:") fail("MALFORMED_EDGE", `${edgePath}: empty link: target`);
    } else {
      checkSnapshotRef(edge.version, `${edgePath}.version`);
    }
  }
}

function checkPackages(packages, path) {
  for (const [key, record] of Object.entries(expectRecord(packages, path))) {
    const recordPath = `${path}.${key}`;
    checkPeerSuffix(key, recordPath);
    if (key.includes("("))
      fail("INVALID_SHAPE", `${recordPath}: package key carries a peer suffix`);
    checkKeys(expectRecord(record, recordPath), PACKAGE_KEYS, recordPath);
    if (!hasOwn(record, "resolution")) fail("INVALID_SHAPE", `${recordPath}: missing resolution`);
    const resolutionRecord = expectRecord(record.resolution, `${recordPath}.resolution`);
    checkKeys(resolutionRecord, RESOLUTION_KEYS, `${recordPath}.resolution`);
    if (typeof resolutionRecord.integrity !== "string") {
      fail("INVALID_SHAPE", `${recordPath}.resolution.integrity must be a string`);
    }
    for (const field of ["cpu", "os", "libc"]) {
      if (hasOwn(record, field)) expectStringArray(record[field], `${recordPath}.${field}`);
    }
    for (const field of ["engines", "peerDependencies", "peerDependenciesMeta"]) {
      if (hasOwn(record, field)) expectRecord(record[field], `${recordPath}.${field}`);
    }
    if (hasOwn(record, "hasBin") && typeof record.hasBin !== "boolean") {
      fail("INVALID_SHAPE", `${recordPath}.hasBin must be a boolean`);
    }
    if (hasOwn(record, "deprecated") && typeof record.deprecated !== "string") {
      fail("INVALID_SHAPE", `${recordPath}.deprecated must be a string`);
    }
  }
}

function checkSnapshots(snapshots, path) {
  for (const [key, record] of Object.entries(expectRecord(snapshots, path))) {
    const recordPath = `${path}.${key}`;
    checkSnapshotRef(key, recordPath);
    checkKeys(expectRecord(record, recordPath), SNAPSHOT_KEYS, recordPath);
    for (const section of SNAPSHOT_EDGE_SECTIONS) {
      if (!hasOwn(record, section)) continue;
      for (const [name, value] of Object.entries(
        expectRecord(record[section], `${recordPath}.${section}`),
      )) {
        checkSnapshotRef(value, `${recordPath}.${section}.${name}`);
      }
    }
    if (hasOwn(record, "optional") && typeof record.optional !== "boolean") {
      fail("INVALID_SHAPE", `${recordPath}.optional must be a boolean`);
    }
    if (hasOwn(record, "transitivePeerDependencies")) {
      expectStringArray(
        record.transitivePeerDependencies,
        `${recordPath}.transitivePeerDependencies`,
      );
    }
  }
}

function checkLockfileVersion(document, label) {
  if (document.lockfileVersion !== LOCKFILE_VERSION) {
    fail("LOCKFILE_VERSION", `${label} lockfileVersion must be the string '${LOCKFILE_VERSION}'`);
  }
}

function checkEnvironmentDocument(environment) {
  checkKeys(environment, ENVIRONMENT_TOP_KEYS, "environment");
  checkLockfileVersion(environment, "environment");
  const importers = expectRecord(environment.importers, "environment.importers");
  const names = Object.keys(importers);
  if (names.length !== 1 || names[0] !== ".") {
    fail("ENVIRONMENT_DOCUMENT", "environment document must have exactly the '.' importer");
  }
  const root = expectRecord(importers["."], "environment.importers['.']");
  checkKeys(root, ENVIRONMENT_IMPORTER_KEYS, "environment.importers['.']");
  if (!hasOwn(root, "packageManagerDependencies")) {
    fail("ENVIRONMENT_DOCUMENT", "environment.importers['.'] lacks packageManagerDependencies");
  }
  for (const section of ENVIRONMENT_IMPORTER_KEYS) {
    if (hasOwn(root, section)) {
      checkImporterEdges(root[section], `environment.importers['.'].${section}`, false);
    }
  }
  checkPackages(environment.packages ?? {}, "environment.packages");
  checkSnapshots(environment.snapshots ?? {}, "environment.snapshots");
}

function checkProjectDocument(project) {
  checkKeys(project, PROJECT_TOP_KEYS, "project");
  checkLockfileVersion(project, "project");
  if (!hasOwn(project, "settings")) fail("PROJECT_DOCUMENT", "project document lacks settings");
  if (!hasOwn(project, "importers")) fail("PROJECT_DOCUMENT", "project document lacks importers");
  expectRecord(project.settings, "project.settings");
  for (const key of ["catalogs", "overrides", "patchedDependencies"]) {
    if (hasOwn(project, key)) expectRecord(project[key], `project.${key}`);
  }
  for (const key of ["pnpmfileChecksum", "packageExtensionsChecksum"]) {
    if (hasOwn(project, key) && typeof project[key] !== "string") {
      fail("INVALID_SHAPE", `project.${key} must be a string`);
    }
  }
  if (hasOwn(project, "ignoredOptionalDependencies")) {
    expectStringArray(project.ignoredOptionalDependencies, "project.ignoredOptionalDependencies");
  }
  for (const [path, importer] of Object.entries(
    expectRecord(project.importers, "project.importers"),
  )) {
    const importerPath = `project.importers['${path}']`;
    if (!isRecord(importer)) fail("INVALID_SHAPE", `${importerPath} must be a mapping`);
    checkKeys(importer, PROJECT_IMPORTER_KEYS, importerPath);
    for (const section of PROJECT_IMPORTER_KEYS) {
      if (hasOwn(importer, section))
        checkImporterEdges(importer[section], `${importerPath}.${section}`, true);
    }
  }
  checkPackages(project.packages ?? {}, "project.packages");
  checkSnapshots(project.snapshots ?? {}, "project.snapshots");
}

export function snapshotKeyFor(name, value) {
  const open = value.indexOf("(");
  const base = open < 0 ? value : value.slice(0, open);
  // An alias edge (`typescript-5.9: typescript@5.9.3`, `vite: '@voidzero-dev/…@1.0.0(…)'`)
  // already names its snapshot; only the part before the peer suffix is inspected,
  // because the suffix itself is full of `name@version` pairs.
  return base.indexOf("@", 1) > 0 ? value : `${name}@${value}`;
}

export function packageKeyOf(snapshotKey) {
  const open = snapshotKey.indexOf("(");
  return open < 0 ? snapshotKey : snapshotKey.slice(0, open);
}

export function resolveLinkTarget(owner, version, importers) {
  const rel = version.slice("link:".length);
  if (rel.length === 0) fail("MALFORMED_EDGE", `${owner}: empty link: target`);
  if (posix.isAbsolute(rel)) fail("LINK_ESCAPE", `${owner}: ${version} is absolute`);
  let target = posix.normalize(posix.join(owner, rel));
  if (target.length > 1 && target.endsWith("/")) target = target.slice(0, -1);
  if (target === ".." || target.startsWith("../")) {
    fail("LINK_ESCAPE", `${owner}: ${version} resolves to ${target}, outside the workspace`);
  }
  if (target === ".")
    fail("LINK_ROOT", `${owner}: ${version} resolves to the excluded root importer`);
  if (!hasOwn(importers, target)) {
    fail(
      "LINK_MISSING_IMPORTER",
      `${owner}: ${version} resolves to ${target}, which is not an importer`,
    );
  }
  return target;
}

function walkSnapshots(startKeys, document, label) {
  const snapshots = document.snapshots ?? {};
  const packages = document.packages ?? {};
  const visited = new Set();
  const queue = [...startKeys];
  while (queue.length > 0) {
    const key = queue.pop();
    if (visited.has(key)) continue;
    if (!hasOwn(snapshots, key))
      fail("MISSING_SNAPSHOT", `${label}: snapshot ${key} is referenced but absent`);
    const packageKey = packageKeyOf(key);
    if (!hasOwn(packages, packageKey)) {
      fail(
        "MISSING_PACKAGE",
        `${label}: package record ${packageKey} is absent for snapshot ${key}`,
      );
    }
    visited.add(key);
    for (const section of SNAPSHOT_EDGE_SECTIONS) {
      for (const [name, value] of Object.entries(snapshots[key][section] ?? {})) {
        queue.push(snapshotKeyFor(name, value));
      }
    }
  }
  return visited;
}

function checkEnvironmentReferences(environment) {
  const root = environment.importers["."];
  const starts = [];
  for (const section of ENVIRONMENT_IMPORTER_KEYS) {
    for (const [name, edge] of Object.entries(root[section] ?? {})) {
      starts.push(snapshotKeyFor(name, edge.version));
    }
  }
  walkSnapshots(starts, environment, "environment");
}

function normalizeSpecifier(specifier, currentVersion) {
  return specifier === `workspace:${currentVersion}`
    ? `workspace:${LOCKSTEP_PLACEHOLDER}`
    : specifier;
}

function selectProjectGraph(project, currentVersion) {
  const importers = project.importers;
  for (const root of SELECTED_IMPORTERS) {
    if (!hasOwn(importers, root)) fail("MISSING_IMPORTER", `selected importer ${root} is absent`);
  }
  const selectedImporters = Object.create(null);
  const snapshotStarts = [];
  const queue = [...SELECTED_IMPORTERS];
  while (queue.length > 0) {
    const path = queue.shift();
    if (hasOwn(selectedImporters, path)) continue;
    const normalized = Object.create(null);
    for (const section of PROJECT_IMPORTER_KEYS) {
      if (!hasOwn(importers[path], section)) continue;
      normalized[section] = Object.create(null);
      for (const [name, edge] of Object.entries(importers[path][section])) {
        normalized[section][name] = {
          specifier: normalizeSpecifier(edge.specifier, currentVersion),
          version: edge.version,
        };
        if (edge.version.startsWith("link:")) {
          queue.push(resolveLinkTarget(path, edge.version, importers));
        } else {
          snapshotStarts.push(snapshotKeyFor(name, edge.version));
        }
      }
    }
    selectedImporters[path] = normalized;
  }
  const reached = walkSnapshots(snapshotStarts, project, "project");
  const snapshots = Object.create(null);
  const packages = Object.create(null);
  for (const key of reached) {
    snapshots[key] = project.snapshots[key];
    packages[packageKeyOf(key)] = project.packages[packageKeyOf(key)];
  }
  return { importers: selectedImporters, snapshots, packages };
}

function optionalFileSha256(path) {
  return existsSync(path) ? sha256(readFileSync(path)) : null;
}

export function collectBuildInputs(importerPaths, repoRoot = defaultRepoRoot) {
  const importers = Object.create(null);
  for (const importer of [...importerPaths].sort()) {
    const manifestPath = join(repoRoot, importer, "package.json");
    let manifest;
    try {
      manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    } catch (error) {
      fail("BUILD_INPUT", `${importer}/package.json is unreadable: ${error.message}`);
    }
    const scripts = manifest.scripts ?? {};
    if (!isRecord(scripts))
      fail("BUILD_INPUT", `${importer}/package.json scripts must be an object`);
    const entry = Object.create(null);
    for (const name of BUILD_SCRIPTS) {
      if (hasOwn(scripts, name) && typeof scripts[name] !== "string") {
        fail("BUILD_INPUT", `${importer}/package.json scripts.${name} must be a string`);
      }
      entry[name] = hasOwn(scripts, name) ? scripts[name] : null;
    }
    entry.tsconfigSha256 = optionalFileSha256(join(repoRoot, importer, "tsconfig.json"));
    importers[importer] = entry;
  }
  return {
    importers,
    tsconfigBaseSha256: optionalFileSha256(join(repoRoot, "tsconfig.base.json")),
  };
}

export function computeLockFingerprint({ lockText, currentVersion, repoRoot = defaultRepoRoot }) {
  if (typeof currentVersion !== "string" || currentVersion.length === 0) {
    fail("INVALID_INPUT", "currentVersion must be the non-empty packages/zfb version");
  }
  const [environment, project] = parseLock(lockText);
  checkEnvironmentDocument(environment);
  checkProjectDocument(project);
  checkEnvironmentReferences(environment);
  const graph = selectProjectGraph(project, currentVersion);

  const projectSections = ["lockfileVersion", "settings", ...PROJECT_OPTIONAL_GLOBAL_KEYS].filter(
    (key) => hasOwn(project, key),
  );
  const canonicalProject = Object.create(null);
  for (const key of projectSections) canonicalProject[key] = project[key];
  Object.assign(canonicalProject, graph);

  const importerPaths = Object.keys(graph.importers).sort();
  const buildInputs = collectBuildInputs(importerPaths, repoRoot);
  const canonical = canonicalJson({
    policyVersion: FINGERPRINT_POLICY_VERSION,
    parser: FINGERPRINT_PARSER,
    environment,
    project: canonicalProject,
    buildInputs,
  });

  return {
    policyVersion: FINGERPRINT_POLICY_VERSION,
    parser: FINGERPRINT_PARSER,
    helperSha256: fingerprintHelperSha256(),
    sha256: sha256(canonical),
    inventory: {
      importers: importerPaths,
      snapshots: Object.keys(graph.snapshots).sort(),
      packages: Object.keys(graph.packages).sort(),
      sections: {
        environment: Object.keys(environment).sort(),
        project: [...projectSections, "importers", "packages", "snapshots"].sort(),
      },
      counts: {
        importers: { reached: importerPaths.length, total: Object.keys(project.importers).length },
        snapshots: {
          reached: Object.keys(graph.snapshots).length,
          total: Object.keys(project.snapshots ?? {}).length,
        },
        packages: {
          reached: Object.keys(graph.packages).length,
          total: Object.keys(project.packages ?? {}).length,
        },
      },
    },
  };
}

function parseArgs(argv) {
  const options = { explain: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--explain") options.explain = true;
    else if (arg === "--lock" || arg === "--version") {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) throw new Error(`${arg} needs a value`);
      options[arg.slice(2)] = value;
      index += 1;
    } else {
      throw new Error(`unknown argument ${arg}`);
    }
  }
  return options;
}

function main(argv) {
  const options = parseArgs(argv);
  const lockPath = resolve(options.lock ?? join(defaultRepoRoot, "pnpm-lock.yaml"));
  const currentVersion =
    options.version ??
    JSON.parse(readFileSync(join(defaultRepoRoot, "packages/zfb/package.json"), "utf8")).version;
  const result = computeLockFingerprint({
    lockText: readFileSync(lockPath, "utf8"),
    currentVersion,
  });
  const output = {
    policyVersion: result.policyVersion,
    parser: result.parser,
    helperSha256: result.helperSha256,
    sha256: result.sha256,
  };
  if (options.explain) output.inventory = result.inventory;
  process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`lock-fingerprint: ${error.message}\n`);
    process.exit(1);
  }
}
