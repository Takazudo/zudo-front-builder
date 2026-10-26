import { afterAll, afterEach, describe, expect, it } from "vite-plus/test";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, normalize, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

import {
  contractPathForPlatform,
  loadRun,
  pnpmLockHashForContract,
  validateIslandSizeBudget,
} from "../../research/v3-island-size/check-budget.mjs";
import { computeLockFingerprint } from "../../research/v3-island-size/lock-fingerprint.mjs";
import {
  IDENTITY_NORMALIZATION_VERSION,
  IDENTITY_NORMALIZED_METRIC,
  NORMALIZED_SIDECAR,
  normalizeMeasurement,
  normalizerSha256,
} from "../../research/v3-island-size/normalize-identity.mjs";
import {
  measuredIdentity,
  measuredTotals,
  proveCargoProducerLock,
  rebaselineContract,
} from "../rebaseline-island-size-linux.mjs";
import { budgetAllowsRetention } from "../island-size-evidence.mjs";
import { asSchemaV2 } from "./fixtures/island-size-checkout.mjs";
import { reviewedCargoLock } from "./fixtures/reviewed-cargo-transition.mjs";

const testDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(testDir, "../..");
const sizeDir = join(repoRoot, "research/v3-island-size");
const realLockText = readFileSync(join(repoRoot, "pnpm-lock.yaml"), "utf8");
const checkedOutVersion = JSON.parse(
  readFileSync(join(repoRoot, "packages/zfb/package.json"), "utf8"),
).version;
const runnerFiles = ["measure-real.mjs", "measure.mjs"];
// Re-derive the checkout-bound input pins (lockfiles and runner sources) so a
// synthetic contract never ages with them; the live-contract tests below keep
// guarding the reviewed pins themselves.
function withCheckoutPins(source) {
  const contract = structuredClone(source);
  contract.toolchain.pnpmLockSha256 = pnpmLockHashForContract(
    realLockText,
    checkedOutVersion,
    contract.toolchain.packageVersion,
  );
  contract.toolchain.cargoLockSha256 = createHash("sha256")
    .update(readFileSync(join(repoRoot, "Cargo.lock")))
    .digest("hex");
  contract.runnerSha256 = Object.fromEntries(
    runnerFiles.map((name) => [
      name,
      createHash("sha256")
        .update(readFileSync(join(sizeDir, name)))
        .digest("hex"),
    ]),
  );
  return contract;
}
// Frozen schemaVersion-1 actual-gzip contract (the Darwin contract before its
// #4169 metric migration). It is the legacy-metric base for the synthetic
// states below.
const baseContract = withCheckoutPins(
  JSON.parse(
    readFileSync(join(testDir, "fixtures/island-size-darwin-actual-gzip-v1.json"), "utf8"),
  ),
);
const fixtures = [
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
const tempRoots = [];
const fingerprintCache = new Map();

function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function checkoutFingerprint(lockText = realLockText) {
  if (!fingerprintCache.has(lockText)) {
    const { policyVersion, parser, helperSha256, sha256 } = computeLockFingerprint({
      lockText,
      currentVersion: checkedOutVersion,
    });
    fingerprintCache.set(lockText, { policyVersion, parser, helperSha256, sha256 });
  }
  return { ...fingerprintCache.get(lockText) };
}

function currentLocalInputs(
  contract = baseContract,
  packageVersion = contract.toolchain.packageVersion,
  lockText = realLockText,
) {
  const contractLockHash = pnpmLockHashForContract(
    lockText,
    checkedOutVersion,
    contract.toolchain.packageVersion,
  );
  return {
    packageVersion,
    fixtureSha256: Object.fromEntries(
      fixtureFiles.map((name) => [
        name,
        sha(readFileSync(join(sizeDir, "fixtures", name + ".tsx"))),
      ]),
    ),
    runnerSha256: Object.fromEntries(
      runnerFiles.map((name) => [name, sha(readFileSync(join(sizeDir, name)))]),
    ),
    pnpmLockSha256: contractLockHash,
    pnpmLockActualSha256:
      packageVersion === contract.toolchain.packageVersion ? contractLockHash : sha(lockText),
    cargoLockSha256: sha(readFileSync(join(repoRoot, "Cargo.lock"))),
    normalizerSha256: normalizerSha256(),
    dependencyFingerprint: checkoutFingerprint(lockText),
    dependencyFingerprintError: null,
  };
}

// Same identity-normalized metric as schemaVersion 2, plus the dependency
// fingerprint that replaces the whole-file lock compare.
function v3Contract(base = baseContract) {
  const contract = structuredClone(base);
  contract.schemaVersion = 3;
  contract.metric = IDENTITY_NORMALIZED_METRIC;
  contract.normalizationVersion = IDENTITY_NORMALIZATION_VERSION;
  contract.normalizerSha256 = normalizerSha256();
  contract.dependencyFingerprint = checkoutFingerprint();
  contract.allowance = { raw: 0, gzipNormalized: 0 };
  for (const mode of ["workspace", "packed"]) {
    for (const [fixture, ceiling] of Object.entries(contract.ceilings[mode])) {
      contract.ceilings[mode][fixture] = { raw: ceiling.raw, gzipNormalized: ceiling.gzip };
    }
  }
  return contract;
}

// A lock edit confined to the excluded docs importer: the whole file moves,
// the dependency fingerprint does not.
function unrelatedLockText() {
  const edited = realLockText.replace(
    /(\n {2}docs:\n {4}dependencies:\n {6}[^\n]+:\n {8}specifier: )([^\n]+)/,
    "$1'$2 || >=0'",
  );
  expect(edited).not.toBe(realLockText);
  return edited;
}

// Synthetic fixtures carry the compiler-owned identity shape the normalizer
// validates: one data-zfb-build token in the HTML and two compiler-shaped
// build sites in the islands entry (crates/zfb-islands/src/esbuild.rs).
function buildToken(fixture) {
  return sha(fixture).slice(0, 16);
}

function filesForFixture(fixture) {
  const token = buildToken(fixture);
  const files = {
    "index.html": Buffer.from(
      fixture === "no-island"
        ? "<!doctype html><html><body>empty</body></html>\n"
        : '<!doctype html><html><head><script type="module" src="/assets/islands-test.js"></script></head><body><div data-zfb-island="Island" data-zfb-build="' +
            token +
            '"></div></body></html>\n',
    ),
  };
  if (fixture === "no-island") return files;

  let entry = "// fixture " + fixture + "\n";
  entry +=
    'function register(n){const manifest={identity:{component:n,build:"' +
    token +
    '"},mount:()=>{let local={component:n,build:"' +
    token +
    '"};return local}};return manifest}\n';
  if (fixture === "event-only") entry += 'void import("./islands-chunk-lazy.js");\n';
  if (fixture === "multi-island") {
    entry += 'import "./shared.js";\n';
    entry += 'void import("./islands-chunk-lazy.js");\n';
  }
  entry += "export {};\n";
  files["assets/islands-test.js"] = Buffer.from(entry);
  if (fixture === "event-only" || fixture === "multi-island") {
    files["assets/islands-chunk-lazy.js"] = Buffer.from(
      "// lazy chunk for " + fixture + "\nexport {};\n",
    );
  }
  if (fixture === "multi-island") {
    files["assets/shared.js"] = Buffer.from("// static shared chunk\nexport {};\n");
  }
  return files;
}

function sortedPaths(files) {
  return Object.keys(files).sort();
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

function summarizeFiles(fixture, files) {
  const paths = sortedPaths(files);
  const jsPaths = paths.filter((path) => /\.(?:js|mjs|cjs)$/.test(path));
  const entryPaths = jsPaths.filter(
    (path) =>
      /^assets\/islands(?:-[^/]+)?\.js$/.test(path) && !path.startsWith("assets/islands-chunk-"),
  );
  const initial = new Set();
  const visit = (path) => {
    if (initial.has(path)) return;
    initial.add(path);
    for (const specifier of staticImports(files[path].toString("utf8"))) {
      const target = normalize(join(dirname(path), specifier)).replaceAll("\\", "/");
      if (files[target]) visit(target);
    }
  };
  for (const path of entryPaths) visit(path);
  for (const match of files["index.html"]
    .toString("utf8")
    .matchAll(/(?:src|href)=["']([^"']+\.(?:js|mjs|cjs))(?:[?#][^"']*)?["']/g)) {
    const path = match[1].replace(/^\//, "").split(/[?#]/, 1)[0];
    if (files[path]) visit(path);
  }
  const chunks = jsPaths.map((file) => {
    const bytes = files[file];
    return {
      file,
      phase: initial.has(file) ? "initial" : "later",
      role: entryPaths.includes(file) ? "entry" : initial.has(file) ? "shared" : "lazy",
      raw: bytes.length,
      gzip: gzipSync(bytes, { level: 9, mtime: 0 }).length,
      sha256: sha(bytes),
    };
  });
  const sum = (rows) => ({
    raw: rows.reduce((total, chunk) => total + chunk.raw, 0),
    gzip: rows.reduce((total, chunk) => total + chunk.gzip, 0),
  });
  const inventory = paths.map((path) => ({ path, sha256: sha(files[path]) }));
  return {
    files: inventory,
    chunks,
    entryPaths,
    initial: sum(chunks.filter((chunk) => chunk.phase === "initial")),
    later: sum(chunks.filter((chunk) => chunk.phase === "later")),
    shipped: sum(chunks),
    repeatVerification: {
      passes: 2,
      inventoriesIdentical: true,
      inventorySha256: sha(Buffer.from(JSON.stringify(inventory))),
    },
  };
}

function writePass(directory, files) {
  for (const [path, bytes] of Object.entries(files)) {
    const absolute = join(directory, path);
    mkdirSync(dirname(absolute), { recursive: true });
    writeFileSync(absolute, bytes);
  }
}

function writeManifest(directory, manifest) {
  mkdirSync(directory, { recursive: true });
  writeFileSync(join(directory, "package.json"), JSON.stringify(manifest, null, 2) + "\n");
}

function packageTarball(manifestBytes) {
  const header = Buffer.alloc(512);
  const writeOctal = (offset, length, value) => {
    header.write(value.toString(8).padStart(length - 1, "0") + "\0", offset, length, "ascii");
  };
  header.write("package/package.json", 0, 100, "utf8");
  writeOctal(100, 8, 0o644);
  writeOctal(108, 8, 0);
  writeOctal(116, 8, 0);
  writeOctal(124, 12, manifestBytes.length);
  writeOctal(136, 12, 0);
  header.fill(0x20, 148, 156);
  header[156] = 0;
  header.write("ustar\0", 257, 6, "ascii");
  header.write("00", 263, 2, "ascii");
  const checksum = header.reduce((total, value) => total + value, 0);
  header.write(checksum.toString(8).padStart(6, "0"), 148, 6, "ascii");
  header[154] = 0;
  header[155] = 0x20;
  const dataLength = Math.ceil(manifestBytes.length / 512) * 512;
  const archive = Buffer.alloc(512 + dataLength + 1024);
  header.copy(archive, 0);
  manifestBytes.copy(archive, 512);
  return gzipSync(archive, { level: 9, mtime: 0 });
}

function makePackageInputs(root, packageVersion = baseContract.toolchain.packageVersion) {
  const packageDirectories = {
    workspace: {
      packagePath: join(root, "packages/workspace/zfb"),
      runtimePackagePath: join(root, "packages/workspace/zfb-runtime"),
    },
    packed: {
      packagePath: join(root, "packages/packed/zfb"),
      runtimePackagePath: join(root, "packages/packed/zfb-runtime"),
    },
  };
  const workspaceZfb = {
    name: "@takazudo/zfb",
    version: packageVersion,
    main: "./src/index.ts",
    publishConfig: { main: "./dist/index.js" },
  };
  const packedZfb = {
    name: "@takazudo/zfb",
    version: packageVersion,
    main: "./dist/index.js",
  };
  const workspaceRuntime = {
    name: "@takazudo/zfb-runtime",
    version: packageVersion,
    main: "./src/index.ts",
    dependencies: { hono: "^4.12.25" },
    publishConfig: { main: "./dist/index.js" },
  };
  const packedRuntime = {
    name: "@takazudo/zfb-runtime",
    version: packageVersion,
    main: "./dist/index.js",
    dependencies: { hono: "^4.12.25" },
  };
  writeManifest(packageDirectories.workspace.packagePath, workspaceZfb);
  writeManifest(packageDirectories.packed.packagePath, packedZfb);
  writeManifest(packageDirectories.workspace.runtimePackagePath, workspaceRuntime);
  writeManifest(packageDirectories.packed.runtimePackagePath, packedRuntime);
  const honoPackagePath = join(root, "node_modules/hono");
  writeManifest(honoPackagePath, { name: "hono", version: "4.12.25" });
  const zfbTarballPath = join(root, `pack/takazudo-zfb-${packageVersion}.tgz`);
  const runtimeTarballPath = join(root, `pack/takazudo-zfb-runtime-${packageVersion}.tgz`);
  mkdirSync(dirname(zfbTarballPath), { recursive: true });
  writeFileSync(
    zfbTarballPath,
    packageTarball(readFileSync(join(packageDirectories.packed.packagePath, "package.json"))),
  );
  writeFileSync(
    runtimeTarballPath,
    packageTarball(
      readFileSync(join(packageDirectories.packed.runtimePackagePath, "package.json")),
    ),
  );
  return {
    workspace: {
      ...packageDirectories.workspace,
      honoPackagePath,
      zfbTarballPath,
      runtimeTarballPath,
    },
    packed: { ...packageDirectories.packed, honoPackagePath, zfbTarballPath, runtimeTarballPath },
  };
}

function provenance(mode, sourceSha, localInputs, packageInputs, platform, schemaVersion) {
  const fingerprint =
    schemaVersion === 3
      ? {
          dependencyFingerprint: {
            policyVersion: localInputs.dependencyFingerprint.policyVersion,
            sha256: localInputs.dependencyFingerprint.sha256,
          },
        }
      : {};
  return {
    sourceSha,
    mode,
    platform,
    fixtureVersion: baseContract.fixtureVersion,
    fixtureSha256: localInputs.fixtureSha256,
    runnerSha256: localInputs.runnerSha256,
    packageVersion: localInputs.packageVersion,
    packageJsonSha256: sha(readFileSync(join(packageInputs.packagePath, "package.json"))),
    packagePath: packageInputs.packagePath,
    runtimePackageVersion: localInputs.packageVersion,
    runtimePackageJsonSha256: sha(
      readFileSync(join(packageInputs.runtimePackagePath, "package.json")),
    ),
    runtimePackagePath: packageInputs.runtimePackagePath,
    honoPackageVersion: "4.12.25",
    honoPackageJsonSha256: sha(readFileSync(join(packageInputs.honoPackagePath, "package.json"))),
    honoPackagePath: packageInputs.honoPackagePath,
    zfbBinarySha256: "d".repeat(64),
    zfbTarballSha256: sha(readFileSync(packageInputs.zfbTarballPath)),
    runtimeTarballSha256: sha(readFileSync(packageInputs.runtimeTarballPath)),
    zfbTarballPath: packageInputs.zfbTarballPath,
    runtimeTarballPath: packageInputs.runtimeTarballPath,
    esbuildVersion: baseContract.toolchain.esbuildVersion,
    esbuildBinarySha256: "e".repeat(64),
    nodeVersion: baseContract.toolchain.nodeVersion,
    zlibVersion: baseContract.toolchain.zlibVersion,
    compression: baseContract.compression,
    repeats: 2,
    pnpmLockSha256: localInputs.pnpmLockActualSha256,
    ...fingerprint,
    cargoLockSha256: localInputs.cargoLockSha256,
  };
}

function normalizedSchema(schemaVersion) {
  return schemaVersion === 2 || schemaVersion === 3;
}

function buildMode(root, mode, sourceSha, localInputs, packageInputs, platform, schemaVersion) {
  const outDir = join(root, mode);
  mkdirSync(outDir, { recursive: true });
  const measurement = {
    provenance: provenance(
      mode,
      sourceSha,
      localInputs,
      packageInputs[mode],
      platform,
      schemaVersion,
    ),
    results: {},
  };
  for (const fixture of fixtures) {
    const files = filesForFixture(fixture);
    const result = summarizeFiles(fixture, files);
    measurement.results[fixture] = result;
    for (const passNumber of [1, 2]) {
      writePass(join(outDir, fixture, "pass-" + passNumber), files);
    }
  }
  writeFileSync(join(outDir, "report.md"), "synthetic measurement report\n");
  const jsonPath = join(outDir, "measurement.json");
  writeFileSync(jsonPath, JSON.stringify(measurement, null, 2) + "\n");
  if (normalizedSchema(schemaVersion)) writeSidecar(jsonPath);
  return { jsonPath, run: loadRun(jsonPath) };
}

// Mirrors `node normalize-identity.mjs --measurement <jsonPath>`.
function writeSidecar(jsonPath) {
  const run = loadRun(jsonPath);
  const sidecar = normalizeMeasurement({
    measurementText: run.measurementText,
    artifacts: run.artifacts,
  });
  // This synthetic saved run models the pinned Linux Node/zlib toolchain even
  // when the workspace test job itself runs under Node 22. Keep the production
  // normalizer and checker strict about the actual toolchain on a live run.
  const pinnedToolchain = JSON.parse(run.measurementText).provenance;
  sidecar.normalizer.nodeVersion = pinnedToolchain.nodeVersion;
  sidecar.normalizer.zlibVersion = pinnedToolchain.zlibVersion;
  writeFileSync(
    join(dirname(jsonPath), NORMALIZED_SIDECAR),
    JSON.stringify(sidecar, null, 2) + "\n",
  );
}

// Building a synthetic state writes and re-reads dozens of small files, and a
// single small-file open costs milliseconds on a security-scanned macOS host.
// Each distinct (platform, contract, version, lock) state is therefore built
// and loaded from disk once as a pristine template; every makeState() call
// returns an isolated in-memory copy and only touches the disk when a test
// reads or rewrites the saved files (see cloneState).
const stateTemplates = new Map();
const templateRoots = [];
let stateCounter = 0;

function makeState(
  platform = baseContract.platform,
  base = baseContract,
  packageVersion = base.toolchain.packageVersion,
  historicalCargoSha = null,
) {
  const key = JSON.stringify([platform, base, packageVersion, historicalCargoSha]);
  if (!stateTemplates.has(key)) {
    stateTemplates.set(key, buildState(platform, base, packageVersion, historicalCargoSha));
  }
  return cloneState(stateTemplates.get(key), packageVersion);
}

// The root only appears in measurement.json's provenance paths, as JSON string
// content, so it is matched in its JSON-escaped form (backslashes on Windows).
function relocatedText(text, templateRoot, root) {
  const escape = (path) => JSON.stringify(path).slice(1, -1);
  return text.replaceAll(escape(templateRoot), escape(root));
}

// A copy of a loaded run that is safe to mutate, relocated from the template
// root to `root`; the text hash and the sidecar's source hash are re-derived.
function relocatedRun(run, templateRoot, root) {
  const measurementText = relocatedText(run.measurementText, templateRoot, root);
  const { artifacts, measurement, measurementText: _text, normalized, ...rest } = run;
  void measurement;
  void _text;
  const copy = structuredClone(rest);
  copy.measurementText = measurementText;
  copy.measurement = JSON.parse(measurementText);
  copy.measurementSha256 = sha(Buffer.from(measurementText, "utf8"));
  if (normalized) {
    copy.normalized = structuredClone(normalized);
    copy.normalized.source.sha256 = copy.measurementSha256;
  }
  copy.artifacts = {};
  for (const [caseDir, entry] of Object.entries(artifacts)) {
    copy.artifacts[caseDir] = {
      passDirectories: [...entry.passDirectories],
      passes: Object.fromEntries(
        Object.entries(entry.passes).map(([pass, value]) => [
          pass,
          {
            entries: structuredClone(value.entries),
            files: Object.fromEntries(
              Object.entries(value.files).map(([path, bytes]) => [path, Buffer.from(bytes)]),
            ),
          },
        ]),
      ),
    };
  }
  return copy;
}

// Written straight from the pristine template, never from the test's mutated
// state; only the relocated text and its sidecar source hash differ.
function writeRunToDisk(templateRun, templateRoot, root, directory) {
  mkdirSync(directory, { recursive: true });
  for (const [fixture, entry] of Object.entries(templateRun.artifacts)) {
    for (const [pass, value] of Object.entries(entry.passes)) {
      writePass(join(directory, fixture, "pass-" + pass), value.files);
    }
  }
  const measurementText = relocatedText(templateRun.measurementText, templateRoot, root);
  writeFileSync(join(directory, "report.md"), "synthetic measurement report\n");
  writeFileSync(join(directory, "measurement.json"), measurementText);
  if (templateRun.normalized) {
    const normalized = structuredClone(templateRun.normalized);
    normalized.source.sha256 = sha(Buffer.from(measurementText, "utf8"));
    writeFileSync(join(directory, NORMALIZED_SIDECAR), JSON.stringify(normalized, null, 2) + "\n");
  }
}

function cloneState(template, packageVersion) {
  const root = join(tmpdir(), "zfb-island-budget-test-" + process.pid + "-" + ++stateCounter);
  const state = {
    sourceSha: template.sourceSha,
    platform: structuredClone(template.platform),
    localInputs: structuredClone(template.localInputs),
    contract: structuredClone(template.contract),
    workspace: relocatedRun(template.workspace, template.root, root),
    packed: relocatedRun(template.packed, template.root, root),
  };
  let disk = null;
  const materialize = () => {
    if (disk) return disk;
    tempRoots.push(root);
    // The name is predictable (pid + counter), so clear a leftover from a killed
    // run with a reused pid and refuse to adopt a directory created meanwhile.
    rmSync(root, { recursive: true, force: true });
    mkdirSync(root);
    const packageInputs = makePackageInputs(root, packageVersion);
    writeRunToDisk(template.workspace, template.root, root, join(root, "workspace"));
    writeRunToDisk(template.packed, template.root, root, join(root, "packed"));
    disk = {
      root,
      packageInputs,
      workspacePath: join(root, "workspace", "measurement.json"),
      packedPath: join(root, "packed", "measurement.json"),
    };
    return disk;
  };
  for (const name of ["root", "packageInputs", "workspacePath", "packedPath"]) {
    Object.defineProperty(state, name, {
      enumerable: true,
      get: () => materialize()[name],
    });
  }
  return state;
}

function buildState(platform, base, packageVersion, historicalCargoSha) {
  const root = mkdtempSync(join(tmpdir(), "zfb-island-budget-template-"));
  templateRoots.push(root);
  const sourceSha = "1".repeat(40);
  const normalized = normalizedSchema(base.schemaVersion);
  const localInputs = currentLocalInputs(base, packageVersion);
  // Synthetic historical identity only; never a live measurement or re-pin.
  if (historicalCargoSha !== null) localInputs.cargoLockSha256 = historicalCargoSha;
  const packageInputs = makePackageInputs(root, packageVersion);
  const workspace = buildMode(
    root,
    "workspace",
    sourceSha,
    localInputs,
    packageInputs,
    platform,
    base.schemaVersion,
  );
  const packed = buildMode(
    root,
    "packed",
    sourceSha,
    localInputs,
    packageInputs,
    platform,
    base.schemaVersion,
  );
  const contract = structuredClone(base);
  for (const mode of ["workspace", "packed"]) {
    const run = mode === "workspace" ? workspace.run : packed.run;
    for (const fixture of fixtures) {
      contract.ceilings[mode][fixture] = normalized
        ? {
            raw: run.normalized.results[fixture].shipped.raw,
            gzipNormalized: run.normalized.results[fixture].shipped.gzipNormalized,
          }
        : { ...run.measurement.results[fixture].shipped };
    }
  }
  return {
    root,
    sourceSha,
    platform,
    localInputs,
    packageInputs,
    contract,
    workspacePath: workspace.jsonPath,
    packedPath: packed.jsonPath,
    workspace: workspace.run,
    packed: packed.run,
  };
}

function validate(state) {
  return validateIslandSizeBudget({
    workspace: state.workspace,
    packed: state.packed,
    contract: state.contract,
    expectedSourceSha: state.sourceSha,
    localInputs: state.localInputs,
    platform: state.platform,
  });
}

function assertHistoricalContractAndReviewedCargoGate(contract) {
  const historical = makeState(
    contract.platform,
    contract,
    contract.toolchain.packageVersion,
    contract.toolchain.cargoLockSha256,
  );
  expect(validate(historical)).toEqual({ errors: [], passed: true });
  // The historical #318 transition is proven from its frozen reviewed bytes,
  // never from the live lock: that proof says nothing about a later candidate.
  const lock = reviewedCargoLock();
  const proof = proveCargoProducerLock({
    contractSha: lock.expectedOldSha,
    artifactSha: lock.expectedNewSha,
    ...lock,
  });
  expect(proof.review.id).toBe("rolldown-native-318");
  const current = makeState(contract.platform, contract);
  if (current.localInputs.cargoLockSha256 === contract.toolchain.cargoLockSha256) {
    expect(validate(current)).toEqual({ errors: [], passed: true });
    return;
  }
  // Policy authorization does NOT admit the new producer. Fresh platform-native
  // bytes and the existing rebaseline gates must move the live contract first.
  expect(validate(current)).toEqual({
    passed: false,
    errors: [
      "local Cargo.lock: SHA-256 differs from the reviewed contract",
      "workspace provenance: Cargo lockfile differs from the contract",
      "packed provenance: Cargo lockfile differs from the contract",
    ],
  });
  // Live admission gate: a lock the contract does not pin stays red until fresh
  // native evidence moves the contract through a reviewed rebaseline.
  expect.fail(
    `live Cargo.lock ${current.localInputs.cargoLockSha256} is not admitted by the ` +
      `${contract.platform.os}-${contract.platform.arch} contract, which pins ` +
      contract.toolchain.cargoLockSha256,
  );
}

function reload(state, mode) {
  const key = mode === "workspace" ? "workspace" : "packed";
  const path = mode === "workspace" ? state.workspacePath : state.packedPath;
  state[key] = loadRun(path);
}

function readPassFiles(directory) {
  const files = {};
  const visit = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const absolute = join(current, entry.name);
      if (entry.isDirectory()) visit(absolute);
      else files[relative(directory, absolute).replaceAll("\\", "/")] = readFileSync(absolute);
    }
  };
  visit(directory);
  return files;
}

function rewriteMeasurement(state, mode) {
  const key = mode === "workspace" ? "workspace" : "packed";
  const jsonPath = mode === "workspace" ? state.workspacePath : state.packedPath;
  const measurement = JSON.parse(readFileSync(jsonPath, "utf8"));
  const resultDir = join(dirname(jsonPath), "event-only", "pass-1");
  measurement.results["event-only"] = summarizeFiles("event-only", readPassFiles(resultDir));
  writeFileSync(jsonPath, JSON.stringify(measurement, null, 2) + "\n");
  if (normalizedSchema(state.contract.schemaVersion)) writeSidecar(jsonPath);
  reload(state, mode);
}

// A synthetic schemaVersion-2 state shaped like the live Linux contract; its
// checkout-bound pins are re-derived, so it exercises the v2 metric and the v2
// whole-file lock compare rather than the reviewed live pins.
function linuxState() {
  const linuxContract = withCheckoutPins(
    asSchemaV2(JSON.parse(readFileSync(join(sizeDir, "decision-linux-x64.json"), "utf8"))),
  );
  return makeState(linuxContract.platform, linuxContract);
}

function v3State() {
  return makeState(baseContract.platform, v3Contract());
}

afterEach(() => {
  for (const root of tempRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

afterAll(() => {
  for (const root of templateRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("island shipped-size budget", () => {
  it("retains the real validator's old-Cargo-only rejection, refusing extra failures", () => {
    // Synthetic saved-byte pair, never a native Linux/Darwin receipt.
    const state = v3State();
    state.contract.toolchain.cargoLockSha256 = "f".repeat(64);
    const result = validate(state);
    expect(result.passed).toBe(false);
    const checkerResult = (result) => ({
      status: 1,
      stderr:
        "Island size budget rejected:\n" +
        result.errors.map((error) => "- " + error + "\n").join(""),
    });
    expect(budgetAllowsRetention(checkerResult(result))).toBe(true);
    state.contract.ceilings.workspace["event-only"].raw = 0;
    expect(budgetAllowsRetention(checkerResult(validate(state)))).toBe(false);
  });

  it("allows lockstep release specifiers but still detects other lockfile drift", () => {
    const lockText = readFileSync(join(repoRoot, "pnpm-lock.yaml"), "utf8");
    const currentVersion = JSON.parse(
      readFileSync(join(repoRoot, "packages/zfb/package.json"), "utf8"),
    ).version;
    const contractHash = pnpmLockHashForContract(
      lockText,
      currentVersion,
      baseContract.toolchain.packageVersion,
    );
    expect(contractHash).toBe(
      sha(
        lockText.replaceAll(
          "specifier: workspace:" + currentVersion,
          "specifier: workspace:" + baseContract.toolchain.packageVersion,
        ),
      ),
    );
    // The reviewed Darwin contract must track this checkout's lockfiles, the
    // same drift guard the Linux contract test enforces through validate().
    // A schemaVersion-3 contract tracks the dependency fingerprint instead of
    // the whole file, so an unrelated lock bump does not redden this check.
    const darwinContract = JSON.parse(readFileSync(join(sizeDir, "decision.json"), "utf8"));
    if (darwinContract.schemaVersion === 3) {
      expect(darwinContract.dependencyFingerprint).toEqual(checkoutFingerprint(lockText));
    } else {
      expect(
        pnpmLockHashForContract(lockText, currentVersion, darwinContract.toolchain.packageVersion),
      ).toBe(darwinContract.toolchain.pnpmLockSha256);
    }
    expect(
      pnpmLockHashForContract(
        lockText.replace("lockfileVersion:", "unexpectedLockfileVersion:"),
        currentVersion,
        baseContract.toolchain.packageVersion,
      ),
    ).not.toBe(contractHash);
  });

  it("checks the current release against the reviewed size ceilings and rejects lockfile drift", () => {
    const currentVersion = JSON.parse(
      readFileSync(join(repoRoot, "packages/zfb/package.json"), "utf8"),
    ).version;
    const state = makeState(baseContract.platform, baseContract, currentVersion);
    expect(validate(state)).toEqual({ errors: [], passed: true });
    state.packed.measurement.provenance.pnpmLockSha256 = "0".repeat(64);
    expect(validate(state).errors).toContain(
      "packed provenance: pnpm lockfile differs from the current checkout",
    );
  });

  it("selects only reviewed Darwin arm64 and Linux x64 contracts", () => {
    expect(contractPathForPlatform({ os: "darwin", arch: "arm64" })).toBe(
      join(sizeDir, "decision.json"),
    );
    expect(contractPathForPlatform({ os: "linux", arch: "x64" })).toBe(
      join(sizeDir, "decision-linux-x64.json"),
    );
    expect(() => contractPathForPlatform({ os: "linux", arch: "arm64" })).toThrow(
      "no reviewed island-size contract for platform linux-arm64",
    );
    const currentPlatformId = process.platform + "-" + process.arch;
    const currentContract = {
      "darwin-arm64": "decision.json",
      "linux-x64": "decision-linux-x64.json",
    }[currentPlatformId];
    if (currentContract) {
      expect(contractPathForPlatform()).toBe(join(sizeDir, currentContract));
    } else {
      expect(() => contractPathForPlatform()).toThrow("no reviewed island-size contract");
    }
  });

  it("records the reviewed Darwin arm64 ceilings with zero allowance", () => {
    const darwinContract = JSON.parse(readFileSync(join(sizeDir, "decision.json"), "utf8"));
    expect(darwinContract.platform).toEqual({ os: "darwin", arch: "arm64" });
    expect(darwinContract.toolchain.packageVersion).toBe("5.1.4");
    expect(darwinContract.schemaVersion).toBe(3);
    expect(darwinContract.metric).toBe(IDENTITY_NORMALIZED_METRIC);
    expect(darwinContract.normalizationVersion).toBe(IDENTITY_NORMALIZATION_VERSION);
    expect(darwinContract.normalizerSha256).toBe(currentLocalInputs().normalizerSha256);
    expect(darwinContract.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(darwinContract.baselineSourceSha).toBe("4ecf8db40745124bf072c37a5fa9f25aa078421e");
    expect(darwinContract.baselineEvidence).toMatchObject({
      event: "local-native",
      localReceipt: { sha256: "29c704ca40f2932d5545d20aae142526fb9c812f9f49ce027d0dd2b4e6693ae6" },
    });
    // pnpmLockSha256 follows the v4.5.3 release-bump (#4295); cargoLockSha256 the
    // reviewed noyalib-4268 producer-lock transition at f65dd70d (#4268,
    // owner-attested local evidence); the ceilings below follow the Rolldown
    // default islands bundler under identity normalizer v2 (#4408, #4417).
    expect(darwinContract.toolchain.pnpmLockSha256).toBe(
      "fa2cbec1b79e90cd7ba6f9a7304ad7ed77a18b4a19e924b83c2924eca1ad44d2",
    );
    expect(darwinContract.toolchain.cargoLockSha256).toBe(
      "216d99d595688e68db223517fe52048f3618d73d36abbb71ea652c25fec97810",
    );
    // rebaseline-island-size-darwin.mjs rewrites this literal once the contract
    // is schemaVersion 3 (#4187); a v2 contract carries no fingerprint.
    if (darwinContract.schemaVersion === 3) {
      expect(darwinContract.dependencyFingerprint.sha256).toBe(
        "e0c6472838fa1f0f7028393c3717a2fa4942fb51ae7cf691146bc535d95f8e95",
      );
    }
    expect(darwinContract.ceilings.workspace).toEqual({
      "no-island": { raw: 0, gzipNormalized: 0 },
      "event-only": { raw: 66691, gzipNormalized: 21252 },
      "scalar-signal": { raw: 66702, gzipNormalized: 21257 },
      "show-for": { raw: 66939, gzipNormalized: 21341 },
      model: { raw: 66720, gzipNormalized: 21258 },
      "blog-theme": { raw: 67145, gzipNormalized: 21391 },
      "json-api": { raw: 66896, gzipNormalized: 21350 },
      "multi-island": { raw: 67022, gzipNormalized: 21352 },
    });
    expect(darwinContract.ceilings.packed).toEqual({
      "no-island": { raw: 0, gzipNormalized: 0 },
      "event-only": { raw: 66691, gzipNormalized: 21252 },
      "scalar-signal": { raw: 66702, gzipNormalized: 21257 },
      "show-for": { raw: 66939, gzipNormalized: 21341 },
      model: { raw: 66720, gzipNormalized: 21258 },
      "blog-theme": { raw: 67145, gzipNormalized: 21391 },
      "json-api": { raw: 66896, gzipNormalized: 21350 },
      "multi-island": { raw: 67022, gzipNormalized: 21352 },
    });
    // Up to the pre-Rolldown contract, raw totals never exceeded the frozen
    // actual-gzip contract's: #4203's runtime rework (observed at run
    // 38451538696) lowered them, and the metric moved. The Rolldown ceilings
    // (#4417) raised raw by 2,143..2,255 bytes per fixture, which puts exactly
    // workspace blog-theme 1 byte above it; gzipNormalized stays below.
    const preRolldown = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-darwin-v4.6.0-noyalib.json"), "utf8"),
    );
    const rawAboveBase = [];
    for (const mode of ["workspace", "packed"]) {
      for (const [fixture, ceiling] of Object.entries(darwinContract.ceilings[mode])) {
        const base = baseContract.ceilings[mode][fixture];
        expect(preRolldown.ceilings[mode][fixture].raw).toBeLessThanOrEqual(base.raw);
        if (ceiling.raw > base.raw)
          rawAboveBase.push(mode + "/" + fixture + " +" + (ceiling.raw - base.raw));
        expect(ceiling.gzipNormalized).toBeLessThanOrEqual(base.gzip);
      }
    }
    expect(rawAboveBase).toEqual(["workspace/blog-theme +1"]);
    assertHistoricalContractAndReviewedCargoGate(darwinContract);
  });

  it("records the reviewed Linux x64 ceilings with zero allowance", () => {
    const linuxContract = JSON.parse(
      readFileSync(join(sizeDir, "decision-linux-x64.json"), "utf8"),
    );
    expect(linuxContract.platform).toEqual({ os: "linux", arch: "x64" });
    expect(linuxContract.toolchain.packageVersion).toBe("5.1.5");
    // A schemaVersion-3 contract tracks the dependency fingerprint, the same
    // drift guard as the Darwin contract above; its whole-file pnpmLockSha256
    // is baseline provenance, pinned by the live release-bump test below.
    expect(linuxContract.schemaVersion).toBe(3);
    expect(linuxContract.dependencyFingerprint).toEqual(checkoutFingerprint());
    expect(linuxContract.metric).toBe(IDENTITY_NORMALIZED_METRIC);
    expect(linuxContract.normalizationVersion).toBe(IDENTITY_NORMALIZATION_VERSION);
    expect(linuxContract.normalizerSha256).toBe(currentLocalInputs().normalizerSha256);
    expect(linuxContract.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(linuxContract.ceilings.workspace).toEqual({
      "no-island": { raw: 0, gzipNormalized: 0 },
      "event-only": { raw: 66691, gzipNormalized: 21252 },
      "scalar-signal": { raw: 66702, gzipNormalized: 21257 },
      "show-for": { raw: 66939, gzipNormalized: 21341 },
      model: { raw: 66720, gzipNormalized: 21258 },
      "blog-theme": { raw: 67145, gzipNormalized: 21391 },
      "json-api": { raw: 66896, gzipNormalized: 21350 },
      "multi-island": { raw: 67022, gzipNormalized: 21352 },
    });
    expect(linuxContract.ceilings.packed).toEqual({
      "no-island": { raw: 0, gzipNormalized: 0 },
      "event-only": { raw: 66691, gzipNormalized: 21252 },
      "scalar-signal": { raw: 66702, gzipNormalized: 21257 },
      "show-for": { raw: 66939, gzipNormalized: 21341 },
      model: { raw: 66720, gzipNormalized: 21258 },
      "blog-theme": { raw: 67145, gzipNormalized: 21391 },
      "json-api": { raw: 66896, gzipNormalized: 21350 },
      "multi-island": { raw: 67022, gzipNormalized: 21352 },
    });
    assertHistoricalContractAndReviewedCargoGate(linuxContract);
  });

  it("accepts a contract written by the release re-pin helper", () => {
    const state = linuxState();
    const totals = measuredTotals({
      workspace: state.workspace.normalized.results,
      packed: state.packed.normalized.results,
    });
    const { contract } = rebaselineContract(
      state.contract,
      totals,
      {
        version: state.contract.toolchain.packageVersion,
        pnpmLockSha256: state.contract.toolchain.pnpmLockSha256,
        normalizerSha256: state.localInputs.normalizerSha256,
        sourceSha: state.sourceSha,
        headSha: state.sourceSha,
        runId: 1,
        jobId: 2,
        runLabel: "Synthetic re-pin",
        identity: measuredIdentity(state.packed.measurement.provenance),
        // Test fixture: a digest-only previous-contract reference (#4219).
        previousContract: { sha256: "d".repeat(64) },
      },
      "release-bump",
    );
    state.contract = contract;
    expect(validate(state)).toEqual({ errors: [], passed: true });
  });

  it("accepts a schemaVersion-3 contract pinned by the dependency fingerprint", () => {
    const state = v3State();
    expect(validate(state)).toEqual({ errors: [], passed: true });
    expect(state.workspace.normalized.metric).toBe(IDENTITY_NORMALIZED_METRIC);
    expect(state.packed.measurement.provenance.dependencyFingerprint).toEqual({
      policyVersion: state.contract.dependencyFingerprint.policyVersion,
      sha256: state.contract.dependencyFingerprint.sha256,
    });

    const overflow = v3State();
    overflow.contract.ceilings.packed.model.gzipNormalized -= 1;
    expect(validate(overflow).errors.join("\n")).toContain(
      "packed model v24.14.0 gzip-identity-normalized v2",
    );

    const withoutSidecar = v3State();
    rmSync(join(dirname(withoutSidecar.workspacePath), NORMALIZED_SIDECAR));
    reload(withoutSidecar, "workspace");
    expect(validate(withoutSidecar).errors.join("\n")).toContain(
      "workspace: metric mismatch: schemaVersion 3 contract pins gzip-identity-normalized ceilings but the measurement has no measurement.normalized.json sidecar",
    );
  });

  it("rejects a schemaVersion-3 contract with a stale dependency fingerprint sha256", () => {
    const state = v3State();
    state.contract.dependencyFingerprint.sha256 = "0".repeat(64);
    expect(validate(state).errors).toEqual([
      "local dependency fingerprint: sha256 differs from the reviewed contract",
    ]);
  });

  it("rejects a schemaVersion-3 contract with a different fingerprint policyVersion", () => {
    const state = v3State();
    state.contract.dependencyFingerprint.policyVersion += 1;
    expect(validate(state).errors).toEqual([
      "local dependency fingerprint: policyVersion differs from the reviewed contract",
    ]);
  });

  it("rejects a schemaVersion-3 contract with a different fingerprint helperSha256", () => {
    const state = v3State();
    state.contract.dependencyFingerprint.helperSha256 = "1".repeat(64);
    expect(validate(state).errors).toEqual([
      "local dependency fingerprint: helperSha256 differs from the reviewed contract; re-pin with `--reason fingerprint-helper --base-ref <ref>`; a changed fingerprint value needs `dependency-graph`/`fingerprint-policy` (#4218)",
    ]);
  });

  it("rejects a schemaVersion-3 contract with a different fingerprint parser", () => {
    const state = v3State();
    state.contract.dependencyFingerprint.parser = "yaml@2.8.0";
    expect(validate(state).errors).toEqual([
      "local dependency fingerprint: parser differs from the reviewed contract",
    ]);
  });

  it("allows dependencyFingerprint only on a schemaVersion-3 contract", () => {
    const v2WithFingerprint = linuxState();
    v2WithFingerprint.contract.dependencyFingerprint = checkoutFingerprint();
    expect(validate(v2WithFingerprint).errors).toContain(
      "contract: schemaVersion 2 cannot declare dependencyFingerprint",
    );

    const v1WithFingerprint = makeState();
    v1WithFingerprint.contract.dependencyFingerprint = checkoutFingerprint();
    expect(validate(v1WithFingerprint).errors).toContain(
      "contract: schemaVersion 1 cannot declare dependencyFingerprint",
    );

    const v3Without = v3State();
    delete v3Without.contract.dependencyFingerprint;
    expect(validate(v3Without).errors).toContain(
      "contract: schemaVersion 3 requires dependencyFingerprint with exactly policyVersion, parser, helperSha256, sha256",
    );

    const v3Malformed = v3State();
    v3Malformed.contract.dependencyFingerprint.policyVersion = "1";
    v3Malformed.contract.dependencyFingerprint.sha256 = "not-a-sha";
    const malformed = validate(v3Malformed).errors;
    expect(malformed).toContain("contract: invalid dependencyFingerprint policyVersion");
    expect(malformed).toContain("contract: invalid dependencyFingerprint sha256");

    const v3BadLock = v3State();
    v3BadLock.contract.toolchain.pnpmLockSha256 = "stale";
    expect(validate(v3BadLock).errors).toContain("contract: invalid toolchain pnpmLockSha256");
  });

  it("rejects run provenance whose dependency fingerprint mismatches the contract schema or checkout", () => {
    const stale = v3State();
    stale.packed.measurement.provenance.dependencyFingerprint.sha256 = "0".repeat(64);
    const staleErrors = validate(stale).errors;
    expect(staleErrors).toContain(
      "packed provenance: dependency fingerprint sha256 differs from the current checkout",
    );
    expect(staleErrors).toContain(
      "workspace and packed runs have mismatched dependencyFingerprint",
    );

    const otherPolicy = v3State();
    otherPolicy.workspace.measurement.provenance.dependencyFingerprint.policyVersion += 1;
    expect(validate(otherPolicy).errors).toContain(
      "workspace provenance: dependency fingerprint policyVersion differs from the current checkout",
    );

    const missing = v3State();
    delete missing.workspace.measurement.provenance.dependencyFingerprint;
    expect(validate(missing).errors).toContain(
      "workspace provenance: schema mismatch: schemaVersion 3 contract requires dependencyFingerprint",
    );

    const extraKey = v3State();
    extraKey.packed.measurement.provenance.dependencyFingerprint.parser = "yaml@2.9.0";
    expect(validate(extraKey).errors).toContain(
      "packed provenance: dependencyFingerprint must list exactly policyVersion and sha256",
    );

    const carriedOnV2 = linuxState();
    carriedOnV2.packed.measurement.provenance.dependencyFingerprint = {
      policyVersion: carriedOnV2.localInputs.dependencyFingerprint.policyVersion,
      sha256: carriedOnV2.localInputs.dependencyFingerprint.sha256,
    };
    expect(validate(carriedOnV2).errors).toContain(
      "packed provenance: schema mismatch: schemaVersion 2 contract cannot accept a run carrying dependencyFingerprint",
    );
  });

  it("rejects workspace and packed runs that disagree on the dependency fingerprint", () => {
    const state = v3State();
    const fingerprint = state.packed.measurement.provenance.dependencyFingerprint;
    state.packed.measurement.provenance.dependencyFingerprint = {
      ...fingerprint,
      sha256: "f".repeat(64),
    };
    expect(validate(state).errors).toEqual([
      "packed provenance: dependency fingerprint sha256 differs from the current checkout",
      "workspace and packed runs have mismatched dependencyFingerprint",
    ]);
  });

  it("ignores an unrelated whole-file lock change on schemaVersion 3 but still pins the run to the checkout", () => {
    const editedLock = unrelatedLockText();
    expect(checkoutFingerprint(editedLock)).toEqual(checkoutFingerprint());
    expect(sha(editedLock)).not.toBe(sha(realLockText));

    const state = v3State();
    const contractVersion = state.contract.toolchain.packageVersion;
    state.localInputs = currentLocalInputs(state.contract, contractVersion, editedLock);
    expect(state.localInputs.pnpmLockSha256).not.toBe(state.contract.toolchain.pnpmLockSha256);
    // The runs were measured against the old lock bytes: byte-exact run-vs-checkout
    // provenance still rejects them even though the contract compare passes.
    expect(validate(state).errors).toEqual([
      "workspace provenance: pnpm lockfile differs from the current checkout",
      "packed provenance: pnpm lockfile differs from the current checkout",
    ]);
    for (const mode of ["workspace", "packed"]) {
      state[mode].measurement.provenance.pnpmLockSha256 = state.localInputs.pnpmLockActualSha256;
    }
    expect(validate(state)).toEqual({ errors: [], passed: true });
  });

  it("keeps the whole-file lock compare for a schemaVersion-2 contract", () => {
    const state = linuxState();
    const contractVersion = state.contract.toolchain.packageVersion;
    state.localInputs = currentLocalInputs(state.contract, contractVersion, unrelatedLockText());
    for (const mode of ["workspace", "packed"]) {
      state[mode].measurement.provenance.pnpmLockSha256 = state.localInputs.pnpmLockActualSha256;
    }
    expect(validate(state).errors).toEqual([
      "local pnpm-lock.yaml: SHA-256 differs from the reviewed contract",
    ]);
  });

  it("compares a schemaVersion-2 contract against gzipNormalized recomputed from bytes", () => {
    const state = linuxState();
    expect(validate(state)).toEqual({ errors: [], passed: true });
    const shipped = state.workspace.normalized.results["multi-island"].shipped;
    expect(shipped.gzipNormalized).toBeGreaterThan(0);
    expect(state.workspace.measurement.results["multi-island"].shipped.gzip).toBe(shipped.gzip);

    const overflow = linuxState();
    overflow.contract.ceilings.workspace["multi-island"].gzipNormalized -= 1;
    const findings = validate(overflow).errors.join("\n");
    expect(findings).toContain("workspace multi-island v24.14.0 gzip-identity-normalized v2");
    expect(findings).toContain("delta 0/+1");
    expect(findings).toContain("actual gzip " + shipped.gzip + ", evidence only");

    const rawOverflow = linuxState();
    rawOverflow.contract.ceilings.packed.model.raw -= 1;
    expect(validate(rawOverflow).errors.join("\n")).toContain("delta +1/0");
  });

  it("leaves gzipNormalized unchanged when only the identity token moves", () => {
    const state = linuxState();
    const before = state.packed.normalized.results["event-only"].shipped;
    const caseDir = join(dirname(state.packedPath), "event-only");
    const oldToken = buildToken("event-only");
    const newToken = "f".repeat(16);
    for (const passNumber of [1, 2]) {
      for (const file of ["index.html", "assets/islands-test.js"]) {
        const path = join(caseDir, "pass-" + passNumber, file);
        writeFileSync(path, readFileSync(path, "utf8").replaceAll(oldToken, newToken));
      }
    }
    const measurement = JSON.parse(readFileSync(state.packedPath, "utf8"));
    measurement.results["event-only"] = summarizeFiles(
      "event-only",
      readPassFiles(join(caseDir, "pass-1")),
    );
    writeFileSync(state.packedPath, JSON.stringify(measurement, null, 2) + "\n");
    writeSidecar(state.packedPath);
    reload(state, "packed");
    const after = state.packed.normalized.results["event-only"];
    expect(after.buildToken).toBe(newToken);
    expect(after.shipped.raw).toBe(before.raw);
    expect(after.shipped.gzipNormalized).toBe(before.gzipNormalized);
    expect(validate(state)).toEqual({ errors: [], passed: true });
  });

  it("fails loudly on a metric mismatch between contract and measurement", () => {
    const legacyWithSidecar = makeState();
    writeSidecar(legacyWithSidecar.workspacePath);
    reload(legacyWithSidecar, "workspace");
    const legacyFindings = validate(legacyWithSidecar).errors.join("\n");
    expect(legacyFindings).toContain(
      "workspace: metric mismatch: schemaVersion 1 contract pins actual gzip ceilings but the measurement carries a gzip-identity-normalized sidecar",
    );

    const normalizedWithoutSidecar = linuxState();
    rmSync(join(dirname(normalizedWithoutSidecar.packedPath), NORMALIZED_SIDECAR));
    reload(normalizedWithoutSidecar, "packed");
    const findings = validate(normalizedWithoutSidecar).errors.join("\n");
    expect(findings).toContain(
      "packed: metric mismatch: schemaVersion 2 contract pins gzip-identity-normalized ceilings but the measurement has no measurement.normalized.json sidecar",
    );

    const legacyShape = linuxState();
    legacyShape.contract.schemaVersion = 1;
    const shapeFindings = validate(legacyShape).errors.join("\n");
    expect(shapeFindings).toContain(
      "contract: schemaVersion 1 measures actual gzip and cannot declare a metric",
    );
    expect(shapeFindings).toContain("contract: allowance must list exactly raw and gzip");
  });

  it("rejects a stale, foreign, or differently versioned normalized sidecar", () => {
    const stale = linuxState();
    stale.packed.normalized.results["model"].shipped.gzipNormalized -= 1;
    expect(validate(stale).errors.join("\n")).toContain(
      "packed normalized: sidecar results are stale against the saved bytes",
    );

    const foreign = linuxState();
    foreign.workspace.normalized.source.sha256 = "0".repeat(64);
    expect(validate(foreign).errors.join("\n")).toContain(
      "workspace normalized: sidecar was produced from a different measurement.json",
    );

    const wrongVersion = linuxState();
    // A sidecar left over from the v1 normalizer (before #4417).
    wrongVersion.workspace.normalized.normalizationVersion = 1;
    expect(validate(wrongVersion).errors.join("\n")).toContain(
      "workspace normalized: normalizationVersion 1 differs from the contract's v2",
    );

    const wrongNormalizer = linuxState();
    wrongNormalizer.contract.normalizerSha256 = "1".repeat(64);
    const findings = validate(wrongNormalizer).errors.join("\n");
    expect(findings).toContain(
      "local normalize-identity.mjs: SHA-256 differs from the reviewed contract",
    );
    expect(findings).toContain("workspace normalized: normalizer hash differs from the contract");

    const wrongNode = linuxState();
    wrongNode.workspace.normalized.normalizer.nodeVersion = "v22.22.2";
    expect(validate(wrongNode).errors.join("\n")).toContain(
      "workspace normalized: sidecar was normalized under an unsupported nodeVersion",
    );

    const tampered = linuxState();
    for (const passNumber of [1, 2]) {
      const entryPath = join(
        dirname(tampered.workspacePath),
        "show-for",
        "pass-" + passNumber,
        "assets/islands-test.js",
      );
      writeFileSync(
        entryPath,
        readFileSync(entryPath, "utf8").replace(buildToken("show-for"), "a".repeat(16)),
      );
    }
    reload(tampered, "workspace");
    expect(validate(tampered).errors.join("\n")).toContain(
      "build identity site carries aaaaaaaaaaaaaaaa but index.html declares " +
        buildToken("show-for"),
    );
  });

  it("rejects a contract or run measured for a different platform", () => {
    const wrongContract = makeState();
    wrongContract.platform = { os: "linux", arch: "x64" };
    const contractErrors = validate(wrongContract).errors.join("\n");
    expect(contractErrors).toContain(
      "contract: platform darwin-arm64 does not match running platform linux-x64",
    );
    expect(contractErrors).toContain(
      "workspace provenance: measured platform differs from the running platform",
    );

    const wrongMeasurement = makeState();
    wrongMeasurement.workspace.measurement.provenance.platform = {
      os: "linux",
      arch: "x64",
    };
    expect(validate(wrongMeasurement).errors.join("\n")).toContain(
      "workspace provenance: measured platform differs from the selected contract",
    );

    const missingMeasurementPlatform = makeState();
    delete missingMeasurementPlatform.workspace.measurement.provenance.platform;
    expect(validate(missingMeasurementPlatform).errors.join("\n")).toContain(
      "workspace provenance: missing or invalid platform provenance",
    );
  });

  it("accepts exact ceilings for both complete eight-fixture matrices", () => {
    const state = makeState();
    const result = validate(state);
    expect(result).toEqual({ errors: [], passed: true });
    expect(state.workspace.measurement.provenance.packageJsonSha256).not.toBe(
      state.packed.measurement.provenance.packageJsonSha256,
    );
    expect(state.workspace.measurement.provenance.runtimePackageJsonSha256).not.toBe(
      state.packed.measurement.provenance.runtimePackageJsonSha256,
    );
    expect(state.workspace.measurement.results["event-only"].later.raw).toBeGreaterThan(0);
    expect(
      state.workspace.measurement.results["multi-island"].chunks.map(({ role }) => role),
    ).toEqual(["lazy", "entry", "shared"]);
    expect(state.workspace.measurement.results["multi-island"].entryPaths).toEqual([
      "assets/islands-test.js",
    ]);
    expect(state.workspace.measurement.results["multi-island"].chunks).toContainEqual(
      expect.objectContaining({
        file: "assets/islands-chunk-lazy.js",
        phase: "later",
        role: "lazy",
      }),
    );
  });

  it("reports raw-only overflow with baseline, actual, delta, and ceiling", () => {
    const state = makeState();
    const size = state.packed.measurement.results["event-only"].shipped;
    state.contract.ceilings.packed["event-only"] = {
      raw: size.raw - 1,
      gzip: size.gzip,
    };
    const result = validate(state);
    expect(result.passed).toBe(false);
    expect(result.errors.join("\n")).toContain("delta +1/0");
  });

  it("reports gzip-only overflow with baseline, actual, delta, and ceiling", () => {
    const state = makeState();
    const size = state.workspace.measurement.results.model.shipped;
    state.contract.ceilings.workspace.model = {
      raw: size.raw,
      gzip: size.gzip - 1,
    };
    const result = validate(state);
    expect(result.passed).toBe(false);
    expect(result.errors.join("\n")).toContain("delta 0/+1");
  });

  it("rejects a missing fixture and an unknown extra fixture", () => {
    const missing = makeState();
    delete missing.workspace.measurement.results["json-api"];
    expect(validate(missing).errors.join("\n")).toContain(
      "workspace: measurement must contain all eight fixture results exactly once",
    );

    const extra = makeState();
    extra.workspace.measurement.results.unreviewed =
      extra.workspace.measurement.results["event-only"];
    extra.workspace.artifacts.unreviewed = extra.workspace.artifacts["event-only"];
    extra.workspace.rootEntries.push("unreviewed");
    expect(validate(extra).errors.join("\n")).toContain(
      "workspace: measurement must contain all eight fixture results exactly once",
    );
  });

  it("rejects duplicate fixture keys in the measurement JSON", () => {
    const state = makeState();
    const text = readFileSync(state.workspacePath, "utf8");
    writeFileSync(
      state.workspacePath,
      text.replace('"results": {', '"results": {\n    "no-island": {},'),
    );
    reload(state, "workspace");
    expect(validate(state).errors.join("\n")).toContain(
      "workspace: duplicate JSON object key $.results.no-island",
    );
  });

  it("rejects missing and unexpected files in saved dist artifacts", () => {
    const missing = makeState();
    delete missing.workspace.artifacts["event-only"].passes["1"].files["assets/islands-test.js"];
    expect(validate(missing).errors.join("\n")).toContain(
      "saved dist inventory differs from measurement.files",
    );

    const extra = makeState();
    extra.packed.artifacts.model.passes["2"].files["assets/unreported.js"] =
      Buffer.from("export {};\n");
    expect(validate(extra).errors.join("\n")).toContain(
      "saved dist inventory differs from measurement.files",
    );
  });

  it("rejects stale file hashes, chunk metadata, and runner provenance", () => {
    const staleFileHash = makeState();
    staleFileHash.workspace.measurement.results["event-only"].files[0].sha256 = "0".repeat(64);
    expect(validate(staleFileHash).errors.join("\n")).toContain("stale SHA-256");

    const staleChunk = makeState();
    staleChunk.packed.measurement.results["event-only"].chunks[0].gzip += 1;
    expect(validate(staleChunk).errors.join("\n")).toContain("stale gzip metadata");

    const staleRunner = makeState();
    staleRunner.workspace.measurement.provenance.runnerSha256["measure-real.mjs"] = "0".repeat(64);
    expect(validate(staleRunner).errors.join("\n")).toContain(
      "workspace provenance: runner hashes differ from the contract",
    );

    const mismatchedTarball = makeState();
    mismatchedTarball.packed.measurement.provenance.zfbTarballSha256 = "9".repeat(64);
    expect(validate(mismatchedTarball).errors.join("\n")).toContain(
      "workspace and packed runs have mismatched zfbTarballSha256",
    );
  });

  it("checks each mode's actual package manifests and tarball files", () => {
    const staleManifest = makeState();
    const workspaceManifestPath = join(
      staleManifest.packageInputs.workspace.packagePath,
      "package.json",
    );
    const workspaceManifest = JSON.parse(readFileSync(workspaceManifestPath, "utf8"));
    workspaceManifest.main = "./src/changed-after-measurement.ts";
    writeFileSync(workspaceManifestPath, JSON.stringify(workspaceManifest, null, 2) + "\n");
    reload(staleManifest, "workspace");
    expect(validate(staleManifest).errors.join("\n")).toContain(
      "workspace provenance: packageJsonSha256 differs from the actual package or tarball",
    );

    const staleTarball = makeState();
    writeFileSync(
      staleTarball.packageInputs.packed.zfbTarballPath,
      packageTarball(
        Buffer.from(
          JSON.stringify({
            name: "@takazudo/zfb",
            version: "3.0.0",
            main: "./dist/changed.js",
          }),
        ),
      ),
    );
    reload(staleTarball, "packed");
    expect(validate(staleTarball).errors.join("\n")).toContain(
      "packed provenance: zfbTarballSha256 differs from the actual package or tarball",
    );
  });

  it("rejects a wrong toolchain version and mismatched requested source SHA", () => {
    const wrongToolchain = makeState();
    wrongToolchain.workspace.measurement.provenance.nodeVersion = "v22.0.0";
    expect(validate(wrongToolchain).errors.join("\n")).toContain(
      "workspace provenance: unsupported Node version",
    );

    const wrongSource = makeState();
    wrongSource.packed.measurement.provenance.sourceSha = "2".repeat(40);
    expect(validate(wrongSource).errors.join("\n")).toContain("packed provenance: source SHA");
  });

  it("counts emitted lazy chunks in the all-shipped ceiling", () => {
    const state = makeState();
    const result = state.packed.measurement.results["event-only"];
    expect(result.later.raw).toBeGreaterThan(0);
    state.contract.ceilings.packed["event-only"] = { ...result.initial };
    const findings = validate(state).errors.join("\n");
    expect(findings).toContain("packed event-only");
    expect(findings).toContain("baseline raw/gzip");
  });

  it("fails on an incompressible payload added to a real temporary JS artifact with fresh metadata", () => {
    const state = makeState();
    const entryPath = "assets/islands-test.js";
    const artifactDir = join(state.workspacePath, "..", "event-only");
    let randomState = 0x3471b00b;
    const randomBytes = Buffer.alloc(3072);
    for (let index = 0; index < randomBytes.length; index += 1) {
      randomState = (Math.imul(randomState, 1664525) + 1013904223) >>> 0;
      randomBytes[index] = randomState >>> 24;
    }
    const payload = Buffer.from(
      "\n// deterministic budget stress " + randomBytes.toString("base64") + "\n",
    );
    for (const passNumber of [1, 2]) {
      const path = join(artifactDir, "pass-" + passNumber, entryPath);
      writeFileSync(path, Buffer.concat([readFileSync(path), payload]));
    }
    rewriteMeasurement(state, "workspace");
    const result = validate(state);
    expect(result.passed).toBe(false);
    expect(result.errors.join("\n")).toContain("workspace event-only");
    expect(result.errors.join("\n")).toContain("delta +");
  });
});

// Historical S7a source remains cited by the dated README corrections below.
const cargoTransitionCommit = "5ea14d57ddda6536a3e661ff07e6547146d7836a";
// Exact immediate predecessors of the live contracts: the noyalib-4268
// producer-lock contracts of both platforms, cited at #4412 merge source 2735135c
// (byte-identical to main 2abb2382) and moved by the cargo-removal Deno exit
// (#4396/#4397), Linux from Actions evidence and Darwin from owner-attested local
// evidence.
// Each must equal a frozen verbatim fixture, so the metadata check also holds in
// shallow CI; full checkouts also compare git bytes and that no intermediate
// contract was skipped. A local-native predecessor has no Actions run, so its
// flat record carries no baselineRunId.
const repinnedFromContracts = [
  {
    // The Linux tokio-security-4569 producer-lock transition cites the v5.1.4
    // runtime-identity contract at 0cfe0d6f0 on the #4569 branch.
    file: "decision-linux-x64.json",
    commit: "0cfe0d6f0afb728890d275bee6839a93e809de65",
    sha256: "0aeadcf44ec603fa9068dd5cf626ee66044e763c7393c0c62219f8cca142bbf9",
    fixture: "fixtures/island-size-linux-x64-v5.1.4-pnpm.json",
    outcome: "runtime-identity-repin",
    packageVersion: "5.1.4",
    baselineSourceSha: "c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283",
    baselineRunId: 40294886927,
  },
  {
    // The Darwin tokio-security-4569 producer-lock transition cites the v5.1.4
    // release contract at 4ecf8db4 on the #4569 branch.
    file: "decision.json",
    commit: "4ecf8db40745124bf072c37a5fa9f25aa078421e",
    sha256: "f23b4969946b6af23349066ece38bb6e951ba76014d75e8f101ffe88acd94113",
    fixture: "fixtures/island-size-darwin-v5.1.4-release.json",
    outcome: "release-version-stamp",
    packageVersion: "5.1.4",
    baselineSourceSha: "c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283",
    baselineRunId: undefined,
  },
];

// The rustls-security-4314 transition (#4314), each platform's before and after
// contract, both frozen: Linux's after is the predecessor of the direct-V8
// rewire; Darwin's is the predecessor of its v4.5.4 release-bump.
const rustlsTransitions = [
  {
    file: "decision-linux-x64.json",
    before: "fixtures/island-size-linux-x64-v4.5.4-release.json",
    after: "fixtures/island-size-linux-x64-v4.5.4-rustls.json",
    // The Linux re-measurement on top of v4.5.4 moved no actual gzip total.
    actualGzipMoved: {},
  },
  {
    file: "decision.json",
    before: "fixtures/island-size-darwin-v4.5.3-rewire.json",
    after: "fixtures/island-size-darwin-v4.5.3-rustls.json",
    // The README records these 11 moved rows (#4314 decision D2).
    actualGzipMoved: {
      workspace: {
        "blog-theme": -3,
        "json-api": -3,
        model: -2,
        "scalar-signal": -3,
        "show-for": 3,
      },
      packed: {
        "blog-theme": 2,
        "event-only": -1,
        "json-api": 4,
        model: 1,
        "multi-island": 3,
        "scalar-signal": -1,
      },
    },
  },
];

// Git reads are spawned synchronously and each costs tens of milliseconds, so
// every answer is computed once per process. A failure is memoized and
// rethrown, exactly as the uncached call would have thrown it.
function memoized(cache, key, compute) {
  if (!cache.has(key)) {
    try {
      cache.set(key, { value: compute() });
    } catch (error) {
      cache.set(key, { error });
    }
  }
  const entry = cache.get(key);
  if (entry.error) throw entry.error;
  return entry.value;
}

const gitBlobCache = new Map();
const gitCommitCache = new Map();
const gitAncestorCache = new Map();
const previousContractCache = new Map();

function gitShowBlob(commit, path) {
  return memoized(gitBlobCache, commit + ":" + path, () =>
    execFileSync("git", ["show", commit + ":" + path], {
      cwd: repoRoot,
      maxBuffer: 64 * 1024 * 1024,
    }),
  );
}

// False in a shallow checkout that lacks the commit.
function gitCommitAvailable(commit) {
  return memoized(gitCommitCache, commit, () => {
    try {
      execFileSync("git", ["cat-file", "-e", commit + "^{commit}"], {
        cwd: repoRoot,
        stdio: "ignore",
      });
      return true;
    } catch {
      return false;
    }
  });
}

// Throws when `commit` is not an ancestor of HEAD.
function assertGitAncestorOfHead(commit) {
  memoized(gitAncestorCache, commit, () => {
    execFileSync("git", ["merge-base", "--is-ancestor", commit, "HEAD"], {
      cwd: repoRoot,
      stdio: "ignore",
    });
    return true;
  });
}

// One `git cat-file --batch` answers every `commit:path` request in order. Each
// response is `<oid> blob <size>\n<size bytes>\n`, or `<request> missing\n`.
function gitShowBlobs(commits, path) {
  const output = execFileSync("git", ["cat-file", "--batch"], {
    cwd: repoRoot,
    input: commits.map((commit) => commit + ":" + path + "\n").join(""),
    maxBuffer: 64 * 1024 * 1024,
  });
  const blobs = [];
  let offset = 0;
  for (const commit of commits) {
    const end = output.indexOf(0x0a, offset);
    if (end === -1) throw new Error("git cat-file --batch: truncated response for " + commit);
    const header = output.toString("utf8", offset, end);
    offset = end + 1;
    const match = /^[0-9a-f]+ blob (\d+)$/.exec(header);
    if (!match) {
      // Missing path or any other non-blob answer: fail where `git show` would.
      blobs.push({ error: new Error("git cat-file --batch: " + header) });
      if (!header.endsWith(" missing")) break;
      continue;
    }
    const size = Number(match[1]);
    if (offset + size + 1 > output.length) {
      throw new Error("git cat-file --batch: truncated blob for " + commit);
    }
    blobs.push({ blob: output.subarray(offset, offset + size) });
    offset += size + 1;
  }
  return blobs;
}

// SHA-256 of the newest committed bytes of `path` that differ from the working
// tree, following history simplification so a merged prerequisite is walked
// through the parent that actually supplied the file.
function previousDistinctContractSha256(path) {
  return memoized(previousContractCache, path, () => {
    const current = sha(readFileSync(join(repoRoot, path)));
    const commits = execFileSync(
      "git",
      ["log", "--topo-order", "--format=%H", "-n", "50", "--", path],
      { cwd: repoRoot, encoding: "utf8" },
    )
      .split("\n")
      .filter(Boolean);
    if (commits.length === 0) return null;
    const results = gitShowBlobs(commits, path);
    for (let index = 0; index < commits.length; index += 1) {
      const result = results[index];
      if (!result) throw new Error("git cat-file --batch: no response for " + commits[index]);
      if (result.error) throw result.error;
      const digest = sha(result.blob);
      if (digest !== current) return digest;
    }
    return null;
  });
}

// A contract without the keys a reviewed Cargo.lock transition may move.
function outsideLockTransitionKeys(contract) {
  const copy = structuredClone(contract);
  for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
    delete copy[key];
  }
  copy.toolchain.cargoLockSha256 = null;
  return copy;
}

describe("live contracts preserve released evidence and authentic predecessors", () => {
  for (const {
    file,
    commit,
    sha256,
    fixture,
    outcome,
    packageVersion,
    baselineSourceSha,
    baselineRunId,
  } of repinnedFromContracts) {
    it(file + " carries a flat repinnedFrom with a verified contract reference", () => {
      const contract = JSON.parse(readFileSync(join(sizeDir, file), "utf8"));
      // The frozen fixture is the cited predecessor, byte for byte, so the
      // metadata check below also holds in a shallow checkout.
      const frozen = readFileSync(join(testDir, fixture));
      expect(sha(frozen)).toBe(sha256);
      const frozenPredecessor = JSON.parse(frozen.toString("utf8"));
      expect(contract.baselineEvidence.repinnedFrom).toEqual({
        schemaVersion: frozenPredecessor.schemaVersion,
        outcome: frozenPredecessor.outcome,
        packageVersion: frozenPredecessor.toolchain.packageVersion,
        baselineSourceSha: frozenPredecessor.baselineSourceSha,
        baselineRunId: frozenPredecessor.baselineEvidence.runId,
        contract: { sha256, commit, path: "research/v3-island-size/" + file },
      });
      const repinnedFrom = contract.baselineEvidence.repinnedFrom;
      expect(Object.keys(repinnedFrom)).toEqual([
        "schemaVersion",
        "outcome",
        "packageVersion",
        "baselineSourceSha",
        ...(baselineRunId === undefined ? [] : ["baselineRunId"]),
        "contract",
      ]);
      expect(repinnedFrom).toMatchObject({
        schemaVersion: 3,
        outcome,
        packageVersion,
        baselineSourceSha,
        ...(baselineRunId === undefined ? {} : { baselineRunId }),
      });
      expect(repinnedFrom).not.toHaveProperty("baselineEvidence");
      expect(repinnedFrom).not.toHaveProperty("repinnedFrom");
      expect(repinnedFrom.contract).toEqual({
        sha256,
        commit,
        path: "research/v3-island-size/" + file,
      });
      // Shallow checkout: the pinned constants above remain mandatory.
      if (gitCommitAvailable(commit)) {
        const blob = gitShowBlob(commit, repinnedFrom.contract.path);
        expect(createHash("sha256").update(blob).digest("hex")).toBe(sha256);
        const predecessor = JSON.parse(blob.toString("utf8"));
        expect(repinnedFrom).toEqual({
          schemaVersion: predecessor.schemaVersion,
          outcome: predecessor.outcome,
          packageVersion: predecessor.toolchain.packageVersion,
          baselineSourceSha: predecessor.baselineSourceSha,
          baselineRunId: predecessor.baselineEvidence.runId,
          contract: { sha256, commit, path: "research/v3-island-size/" + file },
        });
        expect(previousDistinctContractSha256(repinnedFrom.contract.path)).toBe(sha256);
      }
    });
  }

  it("keeps the released v4.5.3 Linux contract as a frozen fixture without stale predecessor metadata", () => {
    // Verbatim bytes of decision-linux-x64.json at released main d70dd6d9.
    const blob = readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.5.3-release.json"));
    expect(createHash("sha256").update(blob).digest("hex")).toBe(
      "08abd3ff01c2927e90121e6bdb98697a62626516d1763e48c13b4f25031ba9cd",
    );
    const contract = JSON.parse(blob.toString("utf8"));
    expect(contract.outcome).toBe("release-version-stamp");
    expect(contract.toolchain.packageVersion).toBe("4.5.3");
    expect(contract.toolchain.cargoLockSha256).toBe(
      "13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37",
    );
    expect(contract.baselineSourceSha).toBe("37b40de3a49fa019d30f414328a42b45e4672d4a");
    expect(contract.baselineEvidence.runId).toBe(38887632282);
    expect(contract.baselineEvidence).not.toHaveProperty("repinnedFrom");
  });

  it("moved the v4.5.3 Linux contract from the released bytes only by the paired cargo-internal-rewire", () => {
    const released = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.5.3-release.json"), "utf8"),
    );
    // Verbatim bytes of decision-linux-x64.json at main 013b0ab8, before the v4.5.4 release bump.
    const blob = readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.5.3-rewire.json"));
    expect(sha(blob)).toBe("f505520217649c5cd0a614fe68c83cd500f44f4aafd8c599082348a2d6e7cf3a");
    const rewired = JSON.parse(blob.toString("utf8"));
    expect(rewired.outcome).toBe("cargo-internal-rewire-transition");
    expect(rewired.toolchain.cargoLockSha256).toBe(
      "230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138",
    );
    expect(rewired.baselineSourceSha).toBe("792c812325310df73540cc20aa590bb8de049d96");
    expect(rewired.baselineEvidence).toMatchObject({
      runId: 39008132372,
      jobId: 117324703148,
      sourceSha: "e2e78eb78c631c351422a66bebf07fbaa00973e2",
      repinnedFrom: {
        outcome: "release-version-stamp",
        baselineRunId: 38887632282,
        contract: { sha256: "08abd3ff01c2927e90121e6bdb98697a62626516d1763e48c13b4f25031ba9cd" },
      },
      cargoRewire: {
        oldSha: "13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37",
        newSha: "230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138",
        declaration: { sha256: "204a8fa17f043503e652a0a8811d07e3143966cfb4ce490ac4429ed0104a369b" },
      },
    });
    // Everything outside the rewire's movable keys is the released contract.
    expect(outsideLockTransitionKeys(rewired)).toEqual(outsideLockTransitionKeys(released));
    expect(rewired.baselineEvidence.actualGzip).toEqual(released.baselineEvidence.actualGzip);
  });

  it("moved the v4.5.4 Linux contract from the v4.5.3 rewire bytes only by the release bump", () => {
    const rewired = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.5.3-rewire.json"), "utf8"),
    );
    // Verbatim decision-linux-x64.json as committed by the v4.5.4 release bump (ea90fe12).
    const blob = readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.5.4-release.json"));
    expect(sha(blob)).toBe("9f8cfcada57773cfabfea3207a41367825ccf63f7239ed506d7781ab056ad211");
    const live = JSON.parse(blob.toString("utf8"));
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("4.5.4");
    expect(live.toolchain.cargoLockSha256).toBe(rewired.toolchain.cargoLockSha256);
    expect(live.baselineSourceSha).toBe("17e69371428fa85d16bd9c9a8fc7781378cc6664");
    expect(live.baselineEvidence).toMatchObject({
      runId: 39465679017,
      jobId: 118776314238,
      sourceSha: "bc2cf771538ceb614be212188300402e25275c2e",
      repinnedFrom: {
        outcome: "cargo-internal-rewire-transition",
        packageVersion: "4.5.3",
        baselineRunId: 39008132372,
        contract: { sha256: "f505520217649c5cd0a614fe68c83cd500f44f4aafd8c599082348a2d6e7cf3a" },
      },
    });
    expect(live.baselineEvidence).not.toHaveProperty("cargoRewire");
    // The v4.5.4 pnpm-lock.yaml the release bump measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "210f0042b072321faf86a5b5229e14cb5f33c1ad0e3c9f882575de0ea926ace2",
    );
    // actualGzip is transfer-size evidence that a version stamp may move, so only
    // its fixture coverage is pinned, not its values.
    const gzipShape = (contract) =>
      Object.fromEntries(
        Object.entries(contract.baselineEvidence.actualGzip).map(([mode, rows]) => [
          mode,
          Object.keys(rows).sort(),
        ]),
      );
    expect(gzipShape(live)).toEqual(gzipShape(rewired));
    // A release bump moves only the version stamp, its lockfile provenance and the evidence.
    const fixed = (contract) => {
      const copy = structuredClone(contract);
      for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
        delete copy[key];
      }
      copy.toolchain.packageVersion = null;
      copy.toolchain.pnpmLockSha256 = null;
      return copy;
    };
    expect(fixed(live)).toEqual(fixed(rewired));
  });

  for (const { file, before, after, actualGzipMoved } of rustlsTransitions) {
    it(
      "moves the frozen " +
        after +
        " from its frozen predecessor only by the rustls-security-4314 transition",
      () => {
        const predecessor = JSON.parse(readFileSync(join(testDir, before), "utf8"));
        const live = JSON.parse(readFileSync(join(testDir, after), "utf8"));
        expect(live.outcome).toBe("cargo-producer-lock-transition");
        expect(predecessor.toolchain.cargoLockSha256).toBe(
          "230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138",
        );
        expect(live.toolchain.cargoLockSha256).toBe(
          "563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd",
        );
        expect(live.baselineEvidence.cargoTransition).toEqual({
          oldSha: predecessor.toolchain.cargoLockSha256,
          newSha: live.toolchain.cargoLockSha256,
          added: ["rustls@0.23.46"],
          changed: [],
          removed: ["rustls@0.23.45"],
          oldLockSource: {
            commit: "21d6807e442bbed61e578c165a91d64c4a7dc141",
            path: "Cargo.lock",
            sha256: predecessor.toolchain.cargoLockSha256,
          },
          review: {
            id: "rustls-security-4314",
            manifestSha256: "fe689aca3a29b012ca7fb96a3aad3dc9fc507f117a18be501e379dae2b3f04ff",
            sourceSha: "21d6807e442bbed61e578c165a91d64c4a7dc141",
          },
        });
        expect(live.baselineEvidence).not.toHaveProperty("cargoRewire");
        if (file === "decision.json") {
          // Owner-attested local native evidence: no Actions run or artifact.
          expect(predecessor.baselineEvidence.event).toBe("ci-artifact");
          expect(live.baselineSourceSha).toBe("bda633b3589e201737b02cf2060f7b24d41a45a7");
          expect(live.baselineEvidence).toMatchObject({
            event: "local-native",
            sourceSha: "bda633b3589e201737b02cf2060f7b24d41a45a7",
            localReceipt: {
              sha256: "52cfabf929b2b9f26c70633daef30d24f135aa8eeef61d6da3e5d65fcd168431",
            },
          });
          for (const key of ["runId", "jobId", "artifact"]) {
            expect(live.baselineEvidence).not.toHaveProperty(key);
          }
        } else {
          expect(live.baselineSourceSha).toBe("949f4cccbdd48ed6d8c8208ec51e93c1dee37b08");
          expect(live.baselineEvidence).toMatchObject({
            event: predecessor.baselineEvidence.event,
            sourceSha: "60d80bcd5e557a141e3fe021336e1054a34b92b4",
            runId: 39570410628,
            jobId: 119122855750,
          });
        }
        expect(outsideLockTransitionKeys(live)).toEqual(outsideLockTransitionKeys(predecessor));
        // Actual gzip is transfer-size evidence, not a ceiling.
        expect(Object.keys(live.baselineEvidence.actualGzip)).toEqual(
          Object.keys(predecessor.baselineEvidence.actualGzip),
        );
        const moved = {};
        for (const [mode, rows] of Object.entries(live.baselineEvidence.actualGzip)) {
          expect(Object.keys(rows)).toEqual(
            Object.keys(predecessor.baselineEvidence.actualGzip[mode]),
          );
          for (const [fixtureName, bytes] of Object.entries(rows)) {
            const delta = bytes - predecessor.baselineEvidence.actualGzip[mode][fixtureName];
            if (delta !== 0) (moved[mode] ??= {})[fixtureName] = delta;
          }
        }
        expect(moved).toEqual(actualGzipMoved);
      },
    );
  }

  it("moves the frozen Darwin v4.5.4 release contract from the frozen rustls contract only by the local release-bump", () => {
    const predecessor = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-darwin-v4.5.3-rustls.json"), "utf8"),
    );
    const live = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-darwin-v4.5.4-release.json"), "utf8"),
    );
    expect(live.outcome).toBe("release-version-stamp");
    expect(predecessor.toolchain.packageVersion).toBe("4.5.3");
    expect(live.toolchain.packageVersion).toBe("4.5.4");
    expect(live.baselineSourceSha).toBe("f9dd1f5f9dad47555cd8ff41b9e65dc0a2e04410");
    // Owner-attested local native evidence (ZFB-L2-022/023): no Actions run or artifact.
    expect(live.baselineEvidence).toMatchObject({
      event: "local-native",
      sourceSha: "f9dd1f5f9dad47555cd8ff41b9e65dc0a2e04410",
      stagingTool: { commit: "3bb16322942eed2f8a49844840c1304ade40187b" },
      localReceipt: {
        sha256: "c5d7f3ced4116cf26265f1c7508be2da221265d429f1f1ab1738ee396197bced",
      },
    });
    for (const key of ["runId", "jobId", "artifact", "cargoTransition", "cargoRewire"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // A release bump moves only the version stamp, its lockfile provenance and the evidence.
    const fixed = (contract) => {
      const copy = structuredClone(contract);
      for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
        delete copy[key];
      }
      copy.toolchain.packageVersion = null;
      copy.toolchain.pnpmLockSha256 = null;
      return copy;
    };
    expect(fixed(live)).toEqual(fixed(predecessor));
  });

  // The paired direct-V8 rewire (ZFB-L2-024 step 2): Linux from health run
  // 39589800240 at merge source 69192eda, Darwin from owner-attested local receipt
  // 9ad4eeaa, both measured at #4368 head c74faf45 with no actual gzip change.
  // Each rewired contract is frozen verbatim, as committed at 9a316700.
  for (const { file, predecessorFixture, rewiredFixture, rewiredSha256, closure } of [
    {
      file: "decision-linux-x64.json",
      predecessorFixture: "fixtures/island-size-linux-x64-v4.5.4-rustls.json",
      rewiredFixture: "fixtures/island-size-linux-x64-v4.5.4-v8rewire.json",
      rewiredSha256: "2625ccc8c12b7aa398421f4c1ed822ce35fbb7d8873df27206c21e2a601d7197",
      closure: "x86_64-unknown-linux-gnu",
    },
    {
      file: "decision.json",
      predecessorFixture: "fixtures/island-size-darwin-v4.5.4-release.json",
      rewiredFixture: "fixtures/island-size-darwin-v4.5.4-v8rewire.json",
      rewiredSha256: "7de9bdfea2a7642aa931f060a8c2764bcece479fa61dcbed4ede98025393b41e",
      closure: "aarch64-apple-darwin",
    },
  ]) {
    it(
      "moved the " +
        file +
        " contract from its frozen predecessor only by the direct-V8 cargo-internal-rewire",
      () => {
        const predecessor = JSON.parse(readFileSync(join(testDir, predecessorFixture), "utf8"));
        const blob = readFileSync(join(testDir, rewiredFixture));
        expect(sha(blob)).toBe(rewiredSha256);
        const live = JSON.parse(blob.toString("utf8"));
        expect(live.outcome).toBe("cargo-internal-rewire-transition");
        expect(predecessor.toolchain.cargoLockSha256).toBe(
          "563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd",
        );
        expect(live.toolchain.cargoLockSha256).toBe(
          "0cd091b7adcc7776895d9de119ac28ac8e8a931cdf28e46c676d5460627cc6f2",
        );
        expect(live.baselineEvidence.cargoRewire).toMatchObject({
          oldSha: predecessor.toolchain.cargoLockSha256,
          newSha: live.toolchain.cargoLockSha256,
          oldLockSource: {
            commit: "f9dd1f5f9dad47555cd8ff41b9e65dc0a2e04410",
            path: "Cargo.lock",
            sha256: predecessor.toolchain.cargoLockSha256,
          },
          newLockSource: {
            commit: "c74faf450160b9eec6c995b5223e172209d62b14",
            path: "Cargo.lock",
            sha256: live.toolchain.cargoLockSha256,
          },
          declaration: {
            path: "prototypes/direct-v8/admission/rewire-declaration.json",
            sha256: "ce15e6cb5f9eff48902b859b2d35060e617bee639d8500440deafb0e751f1dba",
          },
          added: [],
          removed: [],
          closure: { target: closure, added: [] },
        });
        expect(live.baselineEvidence.cargoRewire.changed.map((row) => row.id)).toEqual([
          "zfb-js-host@0.0.0",
        ]);
        expect(live.baselineEvidence).not.toHaveProperty("cargoTransition");
        if (file === "decision.json") {
          expect(live.baselineSourceSha).toBe("c74faf450160b9eec6c995b5223e172209d62b14");
          expect(live.baselineEvidence).toMatchObject({
            event: "local-native",
            sourceSha: "c74faf450160b9eec6c995b5223e172209d62b14",
            stagingTool: { commit: "c74faf450160b9eec6c995b5223e172209d62b14" },
            localReceipt: {
              sha256: "9ad4eeaa6f254cb46e9a887fb0687d7ef9c64cfecae2d6622f532f56636fdd46",
            },
          });
          for (const key of ["runId", "jobId", "artifact"]) {
            expect(live.baselineEvidence).not.toHaveProperty(key);
          }
        } else {
          expect(live.baselineEvidence).toMatchObject({
            event: "ci",
            runId: 39589800240,
            jobId: 119187618804,
            sourceSha: "69192eda2f298444be1043da02d7bc70a8c035d4",
          });
        }
        expect(outsideLockTransitionKeys(live)).toEqual(outsideLockTransitionKeys(predecessor));
        expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
      },
    );
  }

  // The webpki-security-4314 transition (#4314): Linux from health run
  // 39609635178 at merge source dbf93a6c (probe head 9a316700); Darwin from
  // owner-attested local evidence at 5b5f2152. Both after-contracts are frozen:
  // the v4.6.0 release-bump replaced Linux and replaces Darwin next.
  for (const { file, before, after, repinnedFrom, evidence } of [
    {
      file: "decision-linux-x64.json",
      before: "fixtures/island-size-linux-x64-v4.5.4-v8rewire.json",
      after: "fixtures/island-size-linux-x64-v4.5.4-webpki.json",
      repinnedFrom: {
        schemaVersion: 3,
        outcome: "cargo-internal-rewire-transition",
        packageVersion: "4.5.4",
        baselineSourceSha: "c74faf450160b9eec6c995b5223e172209d62b14",
        baselineRunId: 39589800240,
        contract: {
          sha256: "2625ccc8c12b7aa398421f4c1ed822ce35fbb7d8873df27206c21e2a601d7197",
          commit: "9a31670040818ab5019dc0fdb2f7f5d1cc45313e",
          path: "research/v3-island-size/decision-linux-x64.json",
        },
      },
      evidence: {
        baselineSourceSha: "9a31670040818ab5019dc0fdb2f7f5d1cc45313e",
        event: "ci",
        sourceSha: "dbf93a6ce7a74d927e0c0cbd9f86a503bc8bb05c",
        runId: 39609635178,
        jobId: 119254853191,
      },
    },
    {
      file: "decision.json",
      before: "fixtures/island-size-darwin-v4.5.4-v8rewire.json",
      after: "fixtures/island-size-darwin-v4.5.4-webpki.json",
      repinnedFrom: {
        schemaVersion: 3,
        outcome: "cargo-internal-rewire-transition",
        packageVersion: "4.5.4",
        baselineSourceSha: "c74faf450160b9eec6c995b5223e172209d62b14",
        contract: {
          sha256: "7de9bdfea2a7642aa931f060a8c2764bcece479fa61dcbed4ede98025393b41e",
          commit: "5b5f21523818634f8982e0dd3d7fc14885141f2c",
          path: "research/v3-island-size/decision.json",
        },
      },
      evidence: {
        baselineSourceSha: "5b5f21523818634f8982e0dd3d7fc14885141f2c",
        event: "local-native",
        sourceSha: "5b5f21523818634f8982e0dd3d7fc14885141f2c",
        stagingTool: { commit: "5b5f21523818634f8982e0dd3d7fc14885141f2c" },
        localReceipt: {
          sha256: "fdffd9ed251d0ecbb72e38fc7ce6bbcb216f6824bd37cc3d7404299af6950547",
        },
      },
    },
  ]) {
    it(
      "moves the " +
        (after ? "frozen " + after : "live " + file) +
        " from its frozen predecessor only by the webpki-security-4314 transition",
      () => {
        const predecessor = JSON.parse(readFileSync(join(testDir, before), "utf8"));
        const live = JSON.parse(
          readFileSync(after ? join(testDir, after) : join(sizeDir, file), "utf8"),
        );
        expect(live.outcome).toBe("cargo-producer-lock-transition");
        expect(predecessor.toolchain.cargoLockSha256).toBe(
          "0cd091b7adcc7776895d9de119ac28ac8e8a931cdf28e46c676d5460627cc6f2",
        );
        expect(live.toolchain.cargoLockSha256).toBe(
          "d51febae86251fc5b354d81b9505aea21841661150105476029fcc3e6327766d",
        );
        expect(live.baselineEvidence.cargoTransition).toEqual({
          oldSha: predecessor.toolchain.cargoLockSha256,
          newSha: live.toolchain.cargoLockSha256,
          added: ["rustls-webpki@0.103.16", "webpki-roots@1.0.10"],
          changed: [],
          removed: ["rustls-webpki@0.103.15", "webpki-roots@1.0.7"],
          oldLockSource: {
            commit: "0cb911138a4153d759c61fec85ec67ca6789aac2",
            path: "Cargo.lock",
            sha256: predecessor.toolchain.cargoLockSha256,
          },
          review: {
            id: "webpki-security-4314",
            manifestSha256: "f7be894d377eb9224871eaefc73bf4fdb774318e144000e5d96145d090508021",
            sourceSha: "0cb911138a4153d759c61fec85ec67ca6789aac2",
          },
        });
        expect(live.baselineEvidence).not.toHaveProperty("cargoRewire");
        const { baselineSourceSha, ...identity } = evidence;
        expect(live.baselineSourceSha).toBe(baselineSourceSha);
        expect(live.baselineEvidence).toMatchObject(identity);
        if (repinnedFrom) expect(live.baselineEvidence.repinnedFrom).toEqual(repinnedFrom);
        expect(outsideLockTransitionKeys(live)).toEqual(outsideLockTransitionKeys(predecessor));
        expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
      },
    );
  }

  it("moves the frozen v4.6.0 Linux contract from the frozen webpki bytes only by the v4.6.0 release bump", () => {
    const predecessor = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.5.4-webpki.json"), "utf8"),
    );
    const live = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.6.0.json"), "utf8"),
    );
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("4.6.0");
    expect(live.toolchain.cargoLockSha256).toBe(predecessor.toolchain.cargoLockSha256);
    expect(live.baselineSourceSha).toBe("f2e157a5bcb0416c8fa8bc302389fb371994fc67");
    expect(live.baselineEvidence).toMatchObject({
      runId: 39628733086,
      jobId: 119320940279,
      sourceSha: "30e1806d3f0863f4b6726cb36d92ef759084783e",
      repinnedFrom: {
        outcome: "cargo-producer-lock-transition",
        packageVersion: "4.5.4",
        baselineRunId: 39609635178,
        contract: { sha256: "8cfddcedb771c4069f96b81102cc92498798e5a210168f78e040be82fe57cb66" },
      },
    });
    for (const key of ["cargoRewire", "cargoTransition"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The v4.6.0 pnpm-lock.yaml the release probe measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "319e1e32bafd3ea1cf647baefac6cf5cd6e0ae0be692e780b4c761c13fb34ec1",
    );
    // The cited probe head f2e157a5 is off main; the same blob entered main at
    // 5b5f2152, so a full checkout anchors the citation there.
    const mainlineCommit = "5b5f21523818634f8982e0dd3d7fc14885141f2c";
    // Shallow checkout: the frozen fixture digest above remains mandatory.
    if (gitCommitAvailable(mainlineCommit)) {
      assertGitAncestorOfHead(mainlineCommit);
      const blob = gitShowBlob(mainlineCommit, "research/v3-island-size/decision-linux-x64.json");
      expect(sha(blob)).toBe(live.baselineEvidence.repinnedFrom.contract.sha256);
    }
    // actualGzip is transfer-size evidence that a version stamp may move, so only
    // its fixture coverage is pinned, not its values.
    const gzipShape = (contract) =>
      Object.fromEntries(
        Object.entries(contract.baselineEvidence.actualGzip).map(([mode, rows]) => [
          mode,
          Object.keys(rows).sort(),
        ]),
      );
    expect(gzipShape(live)).toEqual(gzipShape(predecessor));
    // A release bump moves only the version stamp, its lockfile provenance and the evidence.
    const fixed = (contract) => {
      const copy = structuredClone(contract);
      for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
        delete copy[key];
      }
      copy.toolchain.packageVersion = null;
      copy.toolchain.pnpmLockSha256 = null;
      return copy;
    };
    expect(fixed(live)).toEqual(fixed(predecessor));
  });

  // The noyalib-4268 transition (#4268): Linux from health run 39661748139 at
  // merge source ad295e2b (probe head cf92bfa5).
  it("moves the frozen noyalib Linux contract from the frozen v4.6.0 bytes only by the noyalib-4268 transition", () => {
    const predecessor = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.6.0.json"), "utf8"),
    );
    const live = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.6.0-noyalib.json"), "utf8"),
    );
    expect(live.outcome).toBe("cargo-producer-lock-transition");
    expect(predecessor.toolchain.cargoLockSha256).toBe(
      "d51febae86251fc5b354d81b9505aea21841661150105476029fcc3e6327766d",
    );
    expect(live.toolchain.cargoLockSha256).toBe(
      "c0d708895c54e312b0f5f9d90ee67b5d1037b9a667ece59d475791a5619e5b64",
    );
    expect(live.baselineEvidence.cargoTransition).toEqual({
      oldSha: predecessor.toolchain.cargoLockSha256,
      newSha: live.toolchain.cargoLockSha256,
      added: ["noyalib-serde-yaml@0.0.58", "noyalib@0.0.58"],
      changed: [],
      removed: ["noyalib-serde-yaml@0.0.55", "noyalib@0.0.55"],
      oldLockSource: {
        commit: "84dc4a842bee059af334a02bfd453d2ce2c74871",
        path: "Cargo.lock",
        sha256: predecessor.toolchain.cargoLockSha256,
      },
      review: {
        id: "noyalib-4268",
        manifestSha256: "5ff33c03ac56e9d494333153d7e5e2cf2e5bf4a2190f0b65fbe145b3c42e1994",
        sourceSha: "84dc4a842bee059af334a02bfd453d2ce2c74871",
      },
    });
    expect(live.baselineEvidence).not.toHaveProperty("cargoRewire");
    expect(live.baselineSourceSha).toBe("cf92bfa545410583215a9829bc5c71781463e14b");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      sourceSha: "ad295e2beec481e9ad1eb8047b1632c9380e06be",
      runId: 39661748139,
      jobId: 119436334916,
    });
    expect(outsideLockTransitionKeys(live)).toEqual(outsideLockTransitionKeys(predecessor));
    expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
  });

  // The noyalib-4268 transition (#4268): Darwin from owner-attested local native
  // evidence at f65dd70d (ZFB-R004 on #4336).
  it("moves the frozen noyalib Darwin contract from the frozen v4.6.0 bytes only by the noyalib-4268 transition", () => {
    const predecessor = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-darwin-v4.6.0.json"), "utf8"),
    );
    const live = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-darwin-v4.6.0-noyalib.json"), "utf8"),
    );
    expect(live.outcome).toBe("cargo-producer-lock-transition");
    expect(predecessor.toolchain.cargoLockSha256).toBe(
      "d51febae86251fc5b354d81b9505aea21841661150105476029fcc3e6327766d",
    );
    expect(live.toolchain.cargoLockSha256).toBe(
      "c0d708895c54e312b0f5f9d90ee67b5d1037b9a667ece59d475791a5619e5b64",
    );
    expect(live.baselineEvidence.cargoTransition).toEqual({
      oldSha: predecessor.toolchain.cargoLockSha256,
      newSha: live.toolchain.cargoLockSha256,
      added: ["noyalib-serde-yaml@0.0.58", "noyalib@0.0.58"],
      changed: [],
      removed: ["noyalib-serde-yaml@0.0.55", "noyalib@0.0.55"],
      oldLockSource: {
        commit: "84dc4a842bee059af334a02bfd453d2ce2c74871",
        path: "Cargo.lock",
        sha256: predecessor.toolchain.cargoLockSha256,
      },
      review: {
        id: "noyalib-4268",
        manifestSha256: "5ff33c03ac56e9d494333153d7e5e2cf2e5bf4a2190f0b65fbe145b3c42e1994",
        sourceSha: "84dc4a842bee059af334a02bfd453d2ce2c74871",
      },
    });
    expect(live.baselineEvidence).not.toHaveProperty("cargoRewire");
    expect(live.baselineSourceSha).toBe("f65dd70d81da0e407fc3261ef57a99c54de83fdd");
    expect(live.baselineEvidence).toMatchObject({
      event: "local-native",
      sourceSha: "f65dd70d81da0e407fc3261ef57a99c54de83fdd",
      stagingTool: { commit: "f65dd70d81da0e407fc3261ef57a99c54de83fdd" },
      localReceipt: { sha256: "ab63a1eee7f0654acea6bcbabae41eb3c4df22d1e89a9e22cc32b63b62394605" },
      originalCheckerResult: { exitCode: 1 },
    });
    expect(outsideLockTransitionKeys(live)).toEqual(outsideLockTransitionKeys(predecessor));
    expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
  });

  // The cargo-removal Deno exit (#4396/#4397): Linux from health run
  // 39739634110 at merge source 2735135c (probe head 9e48145d), Darwin from
  // owner-attested local native evidence at 2735135c on ball-lightning.
  const denoExitRemoval = (predecessor, live) => {
    expect(live.outcome).toBe("cargo-removal-transition");
    expect(predecessor.toolchain.cargoLockSha256).toBe(
      "c0d708895c54e312b0f5f9d90ee67b5d1037b9a667ece59d475791a5619e5b64",
    );
    expect(live.toolchain.cargoLockSha256).toBe(
      "f004a0680082afbfa29e215dc88bacff751da71f0aa80f9362a30c1248630216",
    );
    const removal = live.baselineEvidence.cargoRemoval;
    expect(removal).toMatchObject({
      oldSha: predecessor.toolchain.cargoLockSha256,
      newSha: live.toolchain.cargoLockSha256,
      oldLockSource: {
        commit: "2abb23825ff5a880d761e9c1cdce57b062a2b839",
        path: "Cargo.lock",
        sha256: predecessor.toolchain.cargoLockSha256,
      },
      declaration: {
        path: "research/v3-island-size/cargo-transitions/deno-exit-4303.json",
        sha256: "dc9e474bacf8bab878620b583c3d175f218461063b23629a88d95634ab598778",
      },
      added: [],
    });
    expect(removal.removed).toHaveLength(21);
    expect(removal.prunedExternal.map((entry) => entry.id)).toEqual([
      "num-bigint@0.4.6",
      "tokio@1.52.3",
      "url@2.5.8",
    ]);
    expect(live.baselineEvidence).not.toHaveProperty("cargoTransition");
    expect(live.baselineEvidence).not.toHaveProperty("cargoRewire");
    expect(outsideLockTransitionKeys(live)).toEqual(outsideLockTransitionKeys(predecessor));
    expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
  };

  // Verbatim bytes of both contracts at ce795d6 (origin/main), the Deno-exit
  // state the Rolldown re-pin (#4417) moved.
  const frozenDenoExit = (fixture, sha256) => {
    const blob = readFileSync(join(testDir, fixture));
    expect(sha(blob)).toBe(sha256);
    return JSON.parse(blob.toString("utf8"));
  };
  const linuxDenoExit = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v4.6.0-deno-exit.json",
      "15b2fe05272b7a6ae8afca95d95d055444fe39b2f5b225fea5d42cd4d3065064",
    );
  const darwinDenoExit = () =>
    frozenDenoExit(
      "fixtures/island-size-darwin-v4.6.0-deno-exit.json",
      "70aa246389be5f47535d28d8a4fc664e9911995e0194d770a239d2a8b9011954",
    );

  it("moves the frozen Deno-exit Linux contract from the frozen noyalib bytes only by the Deno-exit removal", () => {
    const predecessor = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-linux-x64-v4.6.0-noyalib.json"), "utf8"),
    );
    const live = linuxDenoExit();
    denoExitRemoval(predecessor, live);
    expect(live.baselineSourceSha).toBe("9e48145db73f95fc5a83140465d85c136e504602");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      sourceSha: "2735135c7634dca7129ae084123d59313365a6d3",
      runId: 39739634110,
      jobId: 119693526763,
    });
  });

  it("moves the frozen Deno-exit Darwin contract from the frozen noyalib bytes only by the Deno-exit removal", () => {
    const predecessor = JSON.parse(
      readFileSync(join(testDir, "fixtures/island-size-darwin-v4.6.0-noyalib.json"), "utf8"),
    );
    const live = darwinDenoExit();
    denoExitRemoval(predecessor, live);
    expect(live.baselineSourceSha).toBe("2735135c7634dca7129ae084123d59313365a6d3");
    expect(live.baselineEvidence).toMatchObject({
      event: "local-native",
      sourceSha: "2735135c7634dca7129ae084123d59313365a6d3",
      stagingTool: { commit: "2735135c7634dca7129ae084123d59313365a6d3" },
      localReceipt: { sha256: "88f9ba2cc6a9170fc91db7d0f6dd6fc1292d55ebecb9ab2a8fde436df0701be9" },
      originalCheckerResult: { exitCode: 1 },
    });
  });

  // The Rolldown default islands bundler (#4408) under identity normalizer v2
  // (#4417, owner decision 2026-10-22): both platforms move to the same measured
  // Rolldown totals, workspace and packed identical, with zero allowance.
  const rolldownCeilings = {
    "no-island": { raw: 0, gzipNormalized: 0 },
    "event-only": { raw: 66691, gzipNormalized: 21252 },
    "scalar-signal": { raw: 66702, gzipNormalized: 21257 },
    "show-for": { raw: 66939, gzipNormalized: 21341 },
    model: { raw: 66720, gzipNormalized: 21258 },
    "blog-theme": { raw: 67145, gzipNormalized: 21391 },
    "json-api": { raw: 66896, gzipNormalized: 21350 },
    "multi-island": { raw: 67022, gzipNormalized: 21352 },
  };
  const rolldownNormalizerV2 = (predecessor, live, measuredSource) => {
    expect(predecessor.normalizationVersion).toBe(1);
    expect(predecessor.normalizerSha256).toBe(
      "ab60382e3c90c7417fc2fbee7429cea8281e90f0d8c8d16a5ecdbccb2e6ae71a",
    );
    expect(live.normalizationVersion).toBe(2);
    expect(live.normalizerSha256).toBe(
      "b07ba2c92f5996ab3586963a1b768dfb3432063a53b10aa65c4f6679e0ce2cec",
    );
    expect(live.normalizerSha256).toBe(normalizerSha256());
    expect(live.ceilings).toEqual({ workspace: rolldownCeilings, packed: rolldownCeilings });
    expect(live.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(live.outcome).toBe("rolldown-ceiling-transition");
    expect(live.platformStatus).toContain("gzip-identity-normalized v2 (#4417)");
    expect(live.platformStatus).toContain("the owner approved the move on 2026-10-22");
    expect(live.platformStatus).toContain(measuredSource);
    expect(live.baselineEvidence.note).toMatch(/^Bundler ceiling transition \(#4417\)/);
    const rest = (contract) => {
      const copy = structuredClone(contract);
      for (const key of [
        "normalizationVersion",
        "normalizerSha256",
        "ceilings",
        "outcome",
        "platformStatus",
      ]) {
        delete copy[key];
      }
      delete copy.baselineEvidence.repinnedFrom;
      delete copy.baselineEvidence.note;
      return copy;
    };
    expect(Object.keys(live)).toEqual(Object.keys(predecessor));
    expect(Object.keys(live.baselineEvidence)).toEqual(Object.keys(predecessor.baselineEvidence));
    expect(rest(live)).toEqual(rest(predecessor));
  };

  // Verbatim bytes of decision-linux-x64.json at fb4c7335 (#4417), the Rolldown
  // state the v4.7.0 release bump moved.
  const linuxRolldown = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v4.6.0-rolldown.json",
      "6ced75c96736805e0fb5b58adb39f4f72298c48086fce8e6c16d7372a73bfe79",
    );

  it("moves the frozen Rolldown Linux contract from the frozen Deno-exit bytes only by the #4417 Rolldown re-pin", () => {
    rolldownNormalizerV2(
      linuxDenoExit(),
      linuxRolldown(),
      "9b518ab3aa1cf354c6a0c153ff2a954381bb26ce",
    );
  });

  // Verbatim bytes of decision-linux-x64.json at b65b83aa, the v4.7.0 release
  // state the runtime-identity re-pin moved.
  const linuxRelease = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v4.7.0-release.json",
      "a70b4377c7011f4e483f9d3416d248aa86ee96232d0758402fec85897fe86794",
    );

  it("moves the frozen v4.7.0 Linux contract from the frozen Rolldown bytes only by the v4.7.0 release bump", () => {
    const predecessor = linuxRolldown();
    const live = linuxRelease();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("4.7.0");
    expect(live.baselineSourceSha).toBe("9f2a25a6354f564ba54fbf1d7f37b02ed2a8ef48");
    expect(live.baselineEvidence).toMatchObject({
      runId: 39823049816,
      jobId: 119979113674,
      sourceSha: "1102644f1d611f2ef80a57ad590262ee51bf9fcc",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The v4.7.0 pnpm-lock.yaml the release probe measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "3f64bb1a06fd4357a7e0498b673964a9f052a8124f039ee413a7fdd3113dc1f1",
    );
    // The cited probe head 9f2a25a6 is off main; the same blob entered main at
    // fb4c7335, so a full checkout anchors the citation there.
    const mainlineCommit = "fb4c73351247b32adef8db11062a9c5cf1dc05a1";
    // Shallow checkout: the frozen fixture digest above remains mandatory.
    if (gitCommitAvailable(mainlineCommit)) {
      assertGitAncestorOfHead(mainlineCommit);
      const blob = gitShowBlob(mainlineCommit, "research/v3-island-size/decision-linux-x64.json");
      expect(sha(blob)).toBe(live.baselineEvidence.repinnedFrom.contract.sha256);
    }
    // actualGzip is transfer-size evidence that a version stamp may move, so only
    // its fixture coverage is pinned, not its values.
    const gzipShape = (contract) =>
      Object.fromEntries(
        Object.entries(contract.baselineEvidence.actualGzip).map(([mode, rows]) => [
          mode,
          Object.keys(rows).sort(),
        ]),
      );
    expect(gzipShape(live)).toEqual(gzipShape(predecessor));
    // A release bump moves only the version stamp, its lockfile provenance and the evidence.
    const fixed = (contract) => {
      const copy = structuredClone(contract);
      for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
        delete copy[key];
      }
      copy.toolchain.packageVersion = null;
      copy.toolchain.pnpmLockSha256 = null;
      return copy;
    };
    expect(fixed(live)).toEqual(fixed(predecessor));
  });

  // Verbatim bytes of decision-linux-x64.json at ebe890c0 (PR #4440), the
  // runtime-identity state the #4429 oxc edge moved.
  const linuxPnpm = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v4.7.0-pnpm.json",
      "295f7e8c40bfa6a08839d1dabfc2b474808fc18295b436475c9e4cc0d9c2932e",
    );

  it("moves the frozen pnpm Linux contract from the frozen v4.7.0 bytes only by the runtime-identity pnpm-lock re-pin", () => {
    const predecessor = linuxRelease();
    const live = linuxPnpm();
    expect(live.outcome).toBe("runtime-identity-repin");
    expect(live.toolchain.packageVersion).toBe("4.7.0");
    expect(live.baselineSourceSha).toBe("b65b83aa7b089145a4d6eef75f83224e8f1cf64a");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 39929245247,
      jobId: 120333413541,
      sourceSha: "b65b83aa7b089145a4d6eef75f83224e8f1cf64a",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The pnpm-lock.yaml at b65b83aa, after #4439 moved it.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "66b1209c8966475f6c96b40fb11a3815a03e92e521118aee4aa80420476fde5b",
    );
    expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
    // This re-pin moves only the pnpm-lock.yaml provenance and the evidence.
    const fixed = (contract) => {
      const copy = structuredClone(contract);
      for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
        delete copy[key];
      }
      copy.toolchain.pnpmLockSha256 = null;
      return copy;
    };
    expect(fixed(live)).toEqual(fixed(predecessor));
  });

  // Verbatim bytes of decision.json at b65b83aa, the Rolldown state the Darwin
  // v4.7.0 release bump moved.
  const darwinRolldown = () =>
    frozenDenoExit(
      "fixtures/island-size-darwin-v4.6.0-rolldown.json",
      "4a6ed98cb765b74ec8ee13303f7aac55ec24cfdbe740bf14826bf5ead8d11d8e",
    );

  it("moves the frozen Rolldown Darwin contract from the frozen Deno-exit bytes only by the #4417 Rolldown re-pin", () => {
    rolldownNormalizerV2(
      darwinDenoExit(),
      darwinRolldown(),
      "e5e49ccfda49c5ed2befb62a45e15700c508f57b",
    );
  });

  // Verbatim bytes of decision.json at ebe890c0 (PR #4440), the v4.7.0 release
  // state the #4429 oxc edge moved.
  const darwinRelease = () =>
    frozenDenoExit(
      "fixtures/island-size-darwin-v4.7.0-release.json",
      "04a4d32600ac4e3bc92db559019348d6fe85039615a8b669f0fb9486e2beb505",
    );

  it("moves the frozen v4.7.0 Darwin contract from the frozen Rolldown bytes only by the v4.7.0 release bump", () => {
    const predecessor = darwinRolldown();
    const live = darwinRelease();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("4.7.0");
    expect(live.baselineSourceSha).toBe("b65b83aa7b089145a4d6eef75f83224e8f1cf64a");
    expect(live.baselineEvidence).toMatchObject({
      event: "local-native",
      sourceSha: "b65b83aa7b089145a4d6eef75f83224e8f1cf64a",
      stagingTool: { commit: "b65b83aa7b089145a4d6eef75f83224e8f1cf64a" },
      localReceipt: { sha256: "26ae987f990a46563399f65bc2e5eb04207253aba8b016fedc4736ddb968fb94" },
      originalCheckerResult: { exitCode: 0 },
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The pnpm-lock.yaml at b65b83aa, after #4439 moved it.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "66b1209c8966475f6c96b40fb11a3815a03e92e521118aee4aa80420476fde5b",
    );
    const gzipShape = (contract) =>
      Object.fromEntries(
        Object.entries(contract.baselineEvidence.actualGzip).map(([mode, rows]) => [
          mode,
          Object.keys(rows).sort(),
        ]),
      );
    expect(gzipShape(live)).toEqual(gzipShape(predecessor));
    // A release bump moves only the version stamp, its lockfile provenance and the evidence.
    const fixed = (contract) => {
      const copy = structuredClone(contract);
      for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
        delete copy[key];
      }
      copy.toolchain.packageVersion = null;
      copy.toolchain.pnpmLockSha256 = null;
      return copy;
    };
    expect(fixed(live)).toEqual(fixed(predecessor));
  });

  // The paired #4429 oxc edge: both live contracts move only Cargo.lock, with
  // the cargoRewire record citing the committed schema-2 declaration.
  const oxcEdge = (predecessor, live) => {
    expect(live.outcome).toBe("cargo-internal-rewire-transition");
    expect(live.baselineSourceSha).toBe("ebe890c02c7e949855774c672da71681ab8944cb");
    expect(predecessor.toolchain.cargoLockSha256).toBe(
      "f004a0680082afbfa29e215dc88bacff751da71f0aa80f9362a30c1248630216",
    );
    expect(live.toolchain.cargoLockSha256).toBe(
      "e79303ad33f123966d23efb32e309f4263afc00e587ea14bab3a2c872273893d",
    );
    expect(live.baselineEvidence.cargoRewire).toMatchObject({
      oldSha: "f004a0680082afbfa29e215dc88bacff751da71f0aa80f9362a30c1248630216",
      newSha: "e79303ad33f123966d23efb32e309f4263afc00e587ea14bab3a2c872273893d",
      oldLockSource: { commit: "7b062415c002151adf20b52aaf77019a90adfa0e", path: "Cargo.lock" },
      newLockSource: { commit: "ebe890c02c7e949855774c672da71681ab8944cb", path: "Cargo.lock" },
      declaration: {
        path: "research/v3-island-size/cargo-transitions/rolldown-oxc-4429.json",
        sha256: "5f6052da3bfc56c6fc9452be08162eca755598123fdc988870f8ee99442ddc63",
      },
      producer: "zfb",
      manifestsAtCommit: "ebe890c02c7e949855774c672da71681ab8944cb",
    });
    for (const key of ["cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
    const fixed = (contract) => {
      const copy = structuredClone(contract);
      for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
        delete copy[key];
      }
      copy.toolchain.cargoLockSha256 = null;
      return copy;
    };
    expect(fixed(live)).toEqual(fixed(predecessor));
  };

  // Verbatim bytes of decision-linux-x64.json at f03535a2 (PR #4440 merged), the
  // #4429 oxc-edge state the v5.0.0 release bump moved.
  const linuxOxc = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v4.7.0-oxc.json",
      "7001ca0374a00f33d61e7cbd59b60ec225701d9270272921bf14887b7f10d719",
    );

  it("moves the frozen oxc Linux contract from the frozen pnpm bytes only by the #4429 oxc edge", () => {
    const live = linuxOxc();
    oxcEdge(linuxPnpm(), live);
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 39936298013,
      jobId: 120356622912,
      sourceSha: "162d31920337b4a7fa1cb60eccc1aeb294129c58",
    });
  });

  // Verbatim bytes of decision-linux-x64.json at c0c73cce (the v5.0.0 release
  // bump), the state the v5.0.1 release bump moved.
  const linuxV500 = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v5.0.0-release.json",
      "bc63f1169867dfac2ccb96bcbb0aec474655196a9082f5d41dfe314fabb7c6bc",
    );

  // A release bump moves only the version stamp, its lockfile provenance and the evidence.
  const expectReleaseBumpOnly = (predecessor, live) => {
    const gzipShape = (contract) =>
      Object.fromEntries(
        Object.entries(contract.baselineEvidence.actualGzip).map(([mode, rows]) => [
          mode,
          Object.keys(rows).sort(),
        ]),
      );
    expect(gzipShape(live)).toEqual(gzipShape(predecessor));
    const fixed = (contract) => {
      const copy = structuredClone(contract);
      for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
        delete copy[key];
      }
      copy.toolchain.packageVersion = null;
      copy.toolchain.pnpmLockSha256 = null;
      return copy;
    };
    expect(fixed(live)).toEqual(fixed(predecessor));
  };

  it("moves the frozen v5.0.0 Linux contract from the frozen oxc bytes only by the v5.0.0 release bump", () => {
    const predecessor = linuxOxc();
    const live = linuxV500();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("5.0.0");
    expect(live.baselineSourceSha).toBe("01833922a79368dc0898f9e0c1e1544daaa3267c");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 39947910044,
      jobId: 120395051310,
      sourceSha: "f9c3842fedc653416ef447d807b8560fad94b0b3",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The v5.0.0 pnpm-lock.yaml the release probe measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "2ef9afb48ac903c6c0bf458c31dbbaec63edfe727935f88c1a610b90439e6585",
    );
    expectReleaseBumpOnly(predecessor, live);
  });

  // Verbatim bytes of decision-linux-x64.json at c9ba39a5 (the v5.0.1 release
  // re-pin on main), the state the v5.1.0 release bump moved.
  const linuxV501 = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v5.0.1-release.json",
      "7839bbe6e9de96258e48d29796db5e560ba0c4a001913d39c5659da539d487a9",
    );

  it("moves the frozen v5.0.1 Linux contract from the frozen v5.0.0 bytes only by the v5.0.1 release bump", () => {
    const predecessor = linuxV500();
    const live = linuxV501();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("5.0.1");
    expect(live.baselineSourceSha).toBe("2f1c821ede49fe0b3faa8c227fdc988c6157d3aa");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 40040309616,
      jobId: 120709611820,
      sourceSha: "48b40cfcd4410884975e65a607af2059d6a68f68",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The v5.0.1 pnpm-lock.yaml the release probe measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "9739bcb3f6ed5eda2b3132029c783465e3d5fd9f7843a805e904bd3e5766bb7b",
    );
    expectReleaseBumpOnly(predecessor, live);
  });

  // Verbatim bytes of decision-linux-x64.json at 97a65452 (the v5.1.0 release
  // bump on main), the state the v5.1.1 release bump moved.
  const linuxV510 = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v5.1.0-release.json",
      "a2a383ede04f821064686ced5db884fd84e87cf5888e161df9dc9b7ea7b24046",
    );

  it("moves the frozen v5.1.0 Linux contract from the frozen v5.0.1 bytes only by the v5.1.0 release bump", () => {
    const predecessor = linuxV501();
    const live = linuxV510();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("5.1.0");
    expect(live.baselineSourceSha).toBe("edc6c102d8768b70aacdc64ff2734f6338e58582");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 40091457859,
      jobId: 120864848114,
      sourceSha: "fffde2e698bd9bcb0f4a468679a0573d5b7ceee6",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The v5.1.0 pnpm-lock.yaml the release probe measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "be97c4c8d50d64cf5ae64e14095a348e89fd7bced1683687bfe98f5dabed9f9b",
    );
    expectReleaseBumpOnly(predecessor, live);
  });

  // Verbatim bytes of decision-linux-x64.json at f860dd48 (the v5.1.1 release
  // bump on main), the state the v5.1.2 release bump moved.
  const linuxV511 = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v5.1.1-release.json",
      "d7c43d861211010793e243f65b593b04eed3ff1aaeb852763c58349078181ccf",
    );

  it("moves the frozen v5.1.1 Linux contract from the frozen v5.1.0 bytes only by the v5.1.1 release bump", () => {
    const predecessor = linuxV510();
    const live = linuxV511();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("5.1.1");
    expect(live.baselineSourceSha).toBe("95ef7d72b733e02f0448027b7027d75e706f64b6");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 40145045250,
      jobId: 121024494368,
      sourceSha: "5ca7f6f3cf7e56066de6a0d3245501a7e7a99dd6",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The v5.1.1 pnpm-lock.yaml the release probe measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "9d96f7012a340f01a5e97ae56b1b2a1dad7ca18512454014d6892e61e2c26f1a",
    );
    expectReleaseBumpOnly(predecessor, live);
  });

  // Verbatim bytes of decision-linux-x64.json at 708ca65d (the v5.1.2 release
  // bump on main), the state the v5.1.3 release bump moved.
  const linuxV512 = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v5.1.2-release.json",
      "6d22f7befb1b69a2f310edaa64466477dc8beececf9aa1cfb8636d6d5ee2ae03",
    );

  it("moves the frozen v5.1.2 Linux contract from the frozen v5.1.1 bytes only by the v5.1.2 release bump", () => {
    const predecessor = linuxV511();
    const live = linuxV512();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("5.1.2");
    expect(live.baselineSourceSha).toBe("6679531ec8c4029b94285015e9775e2d0a135f43");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 40172646987,
      jobId: 121106594309,
      sourceSha: "4e71f7a7d9a9f0b0be96a88bbf8ed04000796600",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The v5.1.2 pnpm-lock.yaml the release probe measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "fdf2ed0c91a21e555c46f3a3a521a13878c50a289751d136042ca5c04fbc1b00",
    );
    expectReleaseBumpOnly(predecessor, live);
  });

  // Verbatim bytes of decision-linux-x64.json at c248f272c (the v5.1.3 release
  // bump on main), the state the v5.1.4 release bump moved.
  const linuxV513 = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v5.1.3-release.json",
      "3e91a8a688375325b70cc21f768cea929a646c2f29aad45c85733817c3e2a623",
    );

  it("moves the frozen v5.1.3 Linux contract from the frozen v5.1.2 bytes only by the v5.1.3 release bump", () => {
    const predecessor = linuxV512();
    const live = linuxV513();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("5.1.3");
    expect(live.baselineSourceSha).toBe("b4cd6c6d58e5f6bac0baadda7fbe6ebc6cbf1745");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 40205487926,
      jobId: 121205423304,
      sourceSha: "ec965d8c4e6535d85553a4198cbb9873bc22a36a",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The v5.1.3 pnpm-lock.yaml the release probe measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "dfc3c1e291c1afb9119363989396184b58be9b461aff53a0756d0145b7ece170",
    );
    expectReleaseBumpOnly(predecessor, live);
  });

  // Verbatim bytes of decision-linux-x64.json at c8a44a5d5 (main after the
  // v5.1.4 release bump), the state the runtime-identity re-pin moved.
  const linuxV514 = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v5.1.4-release.json",
      "96284cc873de3924091b677ebbbd8b49c07c0143b30e12bb4f5044a4e759a1a2",
    );

  it("moves the frozen v5.1.4 Linux contract from the frozen v5.1.3 bytes only by the v5.1.4 release bump", () => {
    const predecessor = linuxV513();
    const live = linuxV514();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("5.1.4");
    expect(live.baselineSourceSha).toBe("4c50217422354f4ee187233e12203cbf60c557c2");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 40242005703,
      jobId: 121315644940,
      sourceSha: "458799c8f919ca51c1f9d7fea4787d8e5b0769f9",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The v5.1.4 pnpm-lock.yaml the release probe measured.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "8ffa0f0bbfb13e379b0afa44e8f88c3ee8e1d82b3a50e706c171ebcb974a80ca",
    );
    expectReleaseBumpOnly(predecessor, live);
  });

  // Verbatim bytes of decision-linux-x64.json at 0cfe0d6f0 (#4569 branch), the
  // runtime-identity state the tokio-security-4569 transition moved.
  const linuxV514Pnpm = () =>
    frozenDenoExit(
      "fixtures/island-size-linux-x64-v5.1.4-pnpm.json",
      "0aeadcf44ec603fa9068dd5cf626ee66044e763c7393c0c62219f8cca142bbf9",
    );

  it("moves the frozen pnpm Linux contract from the frozen v5.1.4 bytes only by the runtime-identity pnpm-lock re-pin", () => {
    const predecessor = linuxV514();
    const live = linuxV514Pnpm();
    expect(live.outcome).toBe("runtime-identity-repin");
    expect(live.toolchain.packageVersion).toBe("5.1.4");
    expect(live.baselineSourceSha).toBe("c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      runId: 40294886927,
      jobId: 121478490915,
      sourceSha: "c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283",
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The pnpm-lock.yaml at c8a44a5d5, after 230f4926c and f4218bb45 moved it.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "fa2cbec1b79e90cd7ba6f9a7304ad7ed77a18b4a19e924b83c2924eca1ad44d2",
    );
    expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
    // This re-pin moves only the pnpm-lock.yaml provenance and the evidence.
    const fixed = (contract) => {
      const copy = structuredClone(contract);
      for (const key of ["outcome", "baselineSourceSha", "platformStatus", "baselineEvidence"]) {
        delete copy[key];
      }
      copy.toolchain.pnpmLockSha256 = null;
      return copy;
    };
    expect(fixed(live)).toEqual(fixed(predecessor));
  });

  // The tokio-security-4569 transition (#4569): Linux from health run
  // 40306538134 at merge source 3bfa2e02 (probe head 66541302).
  it("moves the live Linux contract from the frozen pnpm bytes only by the tokio-security-4569 transition", () => {
    const predecessor = linuxV514Pnpm();
    const live = JSON.parse(readFileSync(join(sizeDir, "decision-linux-x64.json"), "utf8"));
    expect(live.outcome).toBe("cargo-producer-lock-transition");
    expect(predecessor.toolchain.cargoLockSha256).toBe(
      "e79303ad33f123966d23efb32e309f4263afc00e587ea14bab3a2c872273893d",
    );
    expect(live.toolchain.cargoLockSha256).toBe(
      "216d99d595688e68db223517fe52048f3618d73d36abbb71ea652c25fec97810",
    );
    expect(live.baselineEvidence.cargoTransition).toEqual({
      oldSha: predecessor.toolchain.cargoLockSha256,
      newSha: live.toolchain.cargoLockSha256,
      added: ["tokio@1.54.1"],
      changed: [],
      removed: ["tokio@1.52.3"],
      oldLockSource: {
        commit: "c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283",
        path: "Cargo.lock",
        sha256: predecessor.toolchain.cargoLockSha256,
      },
      review: {
        id: "tokio-security-4569",
        manifestSha256: "c399eadde02b884221fbecea61ea6ee51263451be0dcc968d3cdba0a21a20a87",
        sourceSha: "c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283",
      },
    });
    for (const key of ["cargoRewire", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    expect(live.baselineSourceSha).toBe("665413028815504af7c53a0b11ef58d43e2237e4");
    expect(live.baselineEvidence).toMatchObject({
      event: "ci",
      sourceSha: "3bfa2e022a60a97f3f60ac733b970b51f671d5fd",
      runId: 40306538134,
      jobId: 121516985404,
    });
    expect(outsideLockTransitionKeys(live)).toEqual(outsideLockTransitionKeys(predecessor));
    expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
  });

  // Verbatim bytes of decision.json at c8a44a5d (main), the #4429 oxc-edge state
  // the Darwin v5.1.4 release bump moved.
  const darwinOxc = () =>
    frozenDenoExit(
      "fixtures/island-size-darwin-v4.7.0-oxc.json",
      "70e441108ce553cdd1e5f6aa0679877e269ab993f84ef270df1ea0cf75df3a78",
    );

  it("moves the frozen oxc Darwin contract from the frozen v4.7.0 bytes only by the #4429 oxc edge", () => {
    const live = darwinOxc();
    oxcEdge(darwinRelease(), live);
    expect(live.baselineEvidence).toMatchObject({
      event: "local-native",
      sourceSha: "ebe890c02c7e949855774c672da71681ab8944cb",
      stagingTool: { commit: "ebe890c02c7e949855774c672da71681ab8944cb" },
      localReceipt: { sha256: "bb9baad7bab9fb0bcd647b28102f5fbcf4ca0e1239a4598875a777813835974a" },
    });
  });

  // Verbatim bytes of decision.json at 4ecf8db4 (#4569 branch), the v5.1.4
  // release state the tokio-security-4569 transition moved.
  const darwinV514 = () =>
    frozenDenoExit(
      "fixtures/island-size-darwin-v5.1.4-release.json",
      "f23b4969946b6af23349066ece38bb6e951ba76014d75e8f101ffe88acd94113",
    );

  it("moves the frozen v5.1.4 Darwin contract from the frozen oxc bytes only by the v5.1.4 release bump", () => {
    const predecessor = darwinOxc();
    const live = darwinV514();
    expect(live.outcome).toBe("release-version-stamp");
    expect(live.toolchain.packageVersion).toBe("5.1.4");
    expect(live.baselineSourceSha).toBe("c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283");
    expect(live.baselineEvidence).toMatchObject({
      event: "local-native",
      sourceSha: "c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283",
      stagingTool: { commit: "c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283" },
      localReceipt: { sha256: "991d743972ed88c311764fa654559c39984602f5bc243114984e74a42c016565" },
      originalCheckerResult: { exitCode: 0 },
    });
    for (const key of ["cargoRewire", "cargoTransition", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    // The pnpm-lock.yaml at c8a44a5d, measured natively on Darwin.
    expect(live.toolchain.pnpmLockSha256).toBe(
      "fa2cbec1b79e90cd7ba6f9a7304ad7ed77a18b4a19e924b83c2924eca1ad44d2",
    );
    expectReleaseBumpOnly(predecessor, live);
  });

  // The tokio-security-4569 transition (#4569): Darwin from owner-attested local
  // native evidence at 4ecf8db4, after the v5.1.4 release bump.
  it("moves the live Darwin contract from the frozen v5.1.4 bytes only by the tokio-security-4569 transition", () => {
    const predecessor = darwinV514();
    const live = JSON.parse(readFileSync(join(sizeDir, "decision.json"), "utf8"));
    expect(live.outcome).toBe("cargo-producer-lock-transition");
    expect(predecessor.toolchain.cargoLockSha256).toBe(
      "e79303ad33f123966d23efb32e309f4263afc00e587ea14bab3a2c872273893d",
    );
    expect(live.toolchain.cargoLockSha256).toBe(
      "216d99d595688e68db223517fe52048f3618d73d36abbb71ea652c25fec97810",
    );
    expect(live.baselineEvidence.cargoTransition).toEqual({
      oldSha: predecessor.toolchain.cargoLockSha256,
      newSha: live.toolchain.cargoLockSha256,
      added: ["tokio@1.54.1"],
      changed: [],
      removed: ["tokio@1.52.3"],
      oldLockSource: {
        commit: "c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283",
        path: "Cargo.lock",
        sha256: predecessor.toolchain.cargoLockSha256,
      },
      review: {
        id: "tokio-security-4569",
        manifestSha256: "c399eadde02b884221fbecea61ea6ee51263451be0dcc968d3cdba0a21a20a87",
        sourceSha: "c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283",
      },
    });
    for (const key of ["cargoRewire", "cargoRemoval"]) {
      expect(live.baselineEvidence).not.toHaveProperty(key);
    }
    expect(live.baselineSourceSha).toBe("4ecf8db40745124bf072c37a5fa9f25aa078421e");
    expect(live.baselineEvidence).toMatchObject({
      event: "local-native",
      sourceSha: "4ecf8db40745124bf072c37a5fa9f25aa078421e",
      stagingTool: { commit: "4ecf8db40745124bf072c37a5fa9f25aa078421e" },
      localReceipt: { sha256: "29c704ca40f2932d5545d20aae142526fb9c812f9f49ce027d0dd2b4e6693ae6" },
      originalCheckerResult: { exitCode: 1 },
    });
    expect(outsideLockTransitionKeys(live)).toEqual(outsideLockTransitionKeys(predecessor));
    expect(live.baselineEvidence.actualGzip).toEqual(predecessor.baselineEvidence.actualGzip);
  });

  it("keeps the dated README corrections", () => {
    const readme = readFileSync(join(sizeDir, "README.md"), "utf8");
    const notes = [
      "**Correction (2026-10-14).** `./ce865ca3-Cargo.lock` was the operator's local copy of `Cargo.lock` at commit `ce865ca3b3c339bb397b12ad3c8f68593ce3d85a`",
      "was in the Linux contract from `" + cargoTransitionCommit + "`",
      "was in the Darwin contract from `" + cargoTransitionCommit + "`",
      "compares with the final state of the v4.4.0 contract at source `05ab82100e8f15fa714db9fd1fb5b8e51b41abaf`",
      "`dcf1e923` release bump; Linux evidence is health run 38473455816 at PR-merge source `fafd4a2fbf1dca98ec346794fee3563bdbc18317`, which is intentionally distinct from probe head `dee7d83b0ea45bd32dbe0c6a7f91846ffb3335f3`",
      "at `42cb1bf9` `normalizerSha256` did not change",
      "## Rolldown default: identity normalizer v2 and ceilings follow #4417",
      "The owner decided on 2026-10-22, in [#4417]",
    ];
    for (const note of notes) expect(readme).toContain(note);
  });
});
