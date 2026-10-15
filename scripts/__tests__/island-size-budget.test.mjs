import { afterEach, describe, expect, it } from "vite-plus/test";
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
  rebaselineContract,
} from "../rebaseline-island-size-linux.mjs";
import { asSchemaV2 } from "./fixtures/island-size-checkout.mjs";

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

function makeState(
  platform = baseContract.platform,
  base = baseContract,
  packageVersion = base.toolchain.packageVersion,
) {
  const root = mkdtempSync(join(tmpdir(), "zfb-island-budget-test-"));
  tempRoots.push(root);
  const sourceSha = "1".repeat(40);
  const normalized = normalizedSchema(base.schemaVersion);
  const localInputs = currentLocalInputs(base, packageVersion);
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

describe("island shipped-size budget", () => {
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
    expect(darwinContract.toolchain.packageVersion).toBe("4.5.0");
    expect(darwinContract.schemaVersion).toBe(3);
    expect(darwinContract.metric).toBe(IDENTITY_NORMALIZED_METRIC);
    expect(darwinContract.normalizationVersion).toBe(IDENTITY_NORMALIZATION_VERSION);
    expect(darwinContract.normalizerSha256).toBe(currentLocalInputs().normalizerSha256);
    expect(darwinContract.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(darwinContract.baselineSourceSha).toBe("dcf1e9235882e365a63cee9d551c58c9f0c19107");
    expect(darwinContract.baselineEvidence).toMatchObject({
      event: "ci-artifact",
      runId: 38484730560,
      jobId: 115595518482,
      artifact: { id: 11859331067 },
    });
    // pnpmLockSha256 comes from the native v4.4.0 fingerprint migration (#4194);
    // cargoLockSha256 and the ceilings below follow run 38451538696 (#4203, #4150).
    expect(darwinContract.toolchain.pnpmLockSha256).toBe(
      "56e687fe05e0b84949881583381551b060cd357f43b0bf6b6bf0b27c2cc95b9a",
    );
    expect(darwinContract.toolchain.cargoLockSha256).toBe(
      "2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e",
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
      "event-only": { raw: 64535, gzipNormalized: 22195 },
      "scalar-signal": { raw: 64543, gzipNormalized: 22213 },
      "show-for": { raw: 64695, gzipNormalized: 22272 },
      model: { raw: 64534, gzipNormalized: 22200 },
      "blog-theme": { raw: 64890, gzipNormalized: 22334 },
      "json-api": { raw: 64692, gzipNormalized: 22284 },
      "multi-island": { raw: 64814, gzipNormalized: 22286 },
    });
    expect(darwinContract.ceilings.packed).toEqual({
      "no-island": { raw: 0, gzipNormalized: 0 },
      "event-only": { raw: 64548, gzipNormalized: 22199 },
      "scalar-signal": { raw: 64556, gzipNormalized: 22203 },
      "show-for": { raw: 64708, gzipNormalized: 22286 },
      model: { raw: 64547, gzipNormalized: 22209 },
      "blog-theme": { raw: 64903, gzipNormalized: 22347 },
      "json-api": { raw: 64705, gzipNormalized: 22296 },
      "multi-island": { raw: 64827, gzipNormalized: 22294 },
    });
    // Raw totals never exceed the frozen actual-gzip contract's; #4203's runtime
    // rework (observed at run 38451538696) lowered them, and the metric moved.
    for (const mode of ["workspace", "packed"]) {
      for (const [fixture, ceiling] of Object.entries(darwinContract.ceilings[mode])) {
        expect(ceiling.raw).toBeLessThanOrEqual(baseContract.ceilings[mode][fixture].raw);
        expect(ceiling.gzipNormalized).toBeLessThanOrEqual(
          baseContract.ceilings[mode][fixture].gzip,
        );
      }
    }
    const state = makeState(darwinContract.platform, darwinContract);
    expect(validate(state)).toEqual({ errors: [], passed: true });
  });

  it("records the reviewed Linux x64 ceilings with zero allowance", () => {
    const linuxContract = JSON.parse(
      readFileSync(join(sizeDir, "decision-linux-x64.json"), "utf8"),
    );
    expect(linuxContract.platform).toEqual({ os: "linux", arch: "x64" });
    expect(linuxContract.toolchain.packageVersion).toBe("4.5.2");
    expect(linuxContract.schemaVersion).toBe(3);
    expect(linuxContract.metric).toBe(IDENTITY_NORMALIZED_METRIC);
    expect(linuxContract.normalizationVersion).toBe(IDENTITY_NORMALIZATION_VERSION);
    expect(linuxContract.normalizerSha256).toBe(currentLocalInputs().normalizerSha256);
    expect(linuxContract.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(linuxContract.ceilings.workspace).toEqual({
      "no-island": { raw: 0, gzipNormalized: 0 },
      "event-only": { raw: 64535, gzipNormalized: 22195 },
      "scalar-signal": { raw: 64543, gzipNormalized: 22213 },
      "show-for": { raw: 64695, gzipNormalized: 22272 },
      model: { raw: 64534, gzipNormalized: 22200 },
      "blog-theme": { raw: 64890, gzipNormalized: 22334 },
      "json-api": { raw: 64692, gzipNormalized: 22284 },
      "multi-island": { raw: 64814, gzipNormalized: 22286 },
    });
    expect(linuxContract.ceilings.packed).toEqual({
      "no-island": { raw: 0, gzipNormalized: 0 },
      "event-only": { raw: 64548, gzipNormalized: 22199 },
      "scalar-signal": { raw: 64556, gzipNormalized: 22203 },
      "show-for": { raw: 64708, gzipNormalized: 22286 },
      model: { raw: 64547, gzipNormalized: 22209 },
      "blog-theme": { raw: 64903, gzipNormalized: 22347 },
      "json-api": { raw: 64705, gzipNormalized: 22296 },
      "multi-island": { raw: 64827, gzipNormalized: 22294 },
    });
    const state = makeState(linuxContract.platform, linuxContract);
    expect(validate(state)).toEqual({ errors: [], passed: true });
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
      "packed model v24.14.0 gzip-identity-normalized v1",
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
    expect(findings).toContain("workspace multi-island v24.14.0 gzip-identity-normalized v1");
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
    wrongVersion.workspace.normalized.normalizationVersion = 2;
    expect(validate(wrongVersion).errors.join("\n")).toContain(
      "workspace normalized: normalizationVersion 2 differs from the contract's v1",
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
