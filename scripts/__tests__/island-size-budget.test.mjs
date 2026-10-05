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

const testDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(testDir, "../..");
const sizeDir = join(repoRoot, "research/v3-island-size");
const baseContract = JSON.parse(readFileSync(join(sizeDir, "decision.json"), "utf8"));
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
const runnerFiles = ["measure-real.mjs", "measure.mjs"];
const tempRoots = [];

function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function currentLocalInputs(
  contract = baseContract,
  packageVersion = contract.toolchain.packageVersion,
) {
  const lockText = readFileSync(join(repoRoot, "pnpm-lock.yaml"), "utf8");
  const checkedOutVersion = JSON.parse(
    readFileSync(join(repoRoot, "packages/zfb/package.json"), "utf8"),
  ).version;
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
  };
}

function filesForFixture(fixture) {
  const files = {
    "index.html": Buffer.from(
      fixture === "no-island"
        ? "<!doctype html><html><body>empty</body></html>\n"
        : '<!doctype html><html><head><script type="module" src="/assets/islands-test.js"></script></head><body>island</body></html>\n',
    ),
  };
  if (fixture === "no-island") return files;

  let entry = "// fixture " + fixture + "\n";
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

function provenance(mode, sourceSha, localInputs, packageInputs, platform) {
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
    cargoLockSha256: localInputs.cargoLockSha256,
  };
}

function buildMode(root, mode, sourceSha, localInputs, packageInputs, platform) {
  const outDir = join(root, mode);
  mkdirSync(outDir, { recursive: true });
  const measurement = {
    provenance: provenance(mode, sourceSha, localInputs, packageInputs[mode], platform),
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
  return { jsonPath, run: loadRun(jsonPath) };
}

function makeState(
  platform = baseContract.platform,
  base = baseContract,
  packageVersion = base.toolchain.packageVersion,
) {
  const root = mkdtempSync(join(tmpdir(), "zfb-island-budget-test-"));
  tempRoots.push(root);
  const sourceSha = "1".repeat(40);
  const localInputs = currentLocalInputs(base, packageVersion);
  const packageInputs = makePackageInputs(root, packageVersion);
  const workspace = buildMode(root, "workspace", sourceSha, localInputs, packageInputs, platform);
  const packed = buildMode(root, "packed", sourceSha, localInputs, packageInputs, platform);
  const contract = structuredClone(base);
  for (const mode of ["workspace", "packed"]) {
    const run = mode === "workspace" ? workspace.run : packed.run;
    for (const fixture of fixtures) {
      contract.ceilings[mode][fixture] = {
        ...run.measurement.results[fixture].shipped,
      };
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
  reload(state, mode);
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
    expect(contractHash).toBe(baseContract.toolchain.pnpmLockSha256);
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

  it("records the reviewed Linux x64 ceilings with zero allowance", () => {
    const linuxContract = JSON.parse(
      readFileSync(join(sizeDir, "decision-linux-x64.json"), "utf8"),
    );
    expect(linuxContract.platform).toEqual({ os: "linux", arch: "x64" });
    expect(linuxContract.toolchain.packageVersion).toBe("3.2.0");
    expect(linuxContract.allowance).toEqual({ raw: 0, gzip: 0 });
    expect(linuxContract.ceilings.workspace).toEqual({
      "no-island": { raw: 0, gzip: 0 },
      "event-only": { raw: 63212, gzip: 21137 },
      "scalar-signal": { raw: 63220, gzip: 21136 },
      "show-for": { raw: 63366, gzip: 21216 },
      model: { raw: 63211, gzip: 21142 },
      "blog-theme": { raw: 63561, gzip: 21279 },
      "json-api": { raw: 63366, gzip: 21230 },
      "multi-island": { raw: 63488, gzip: 21238 },
    });
    expect(linuxContract.ceilings.packed).toEqual({
      "no-island": { raw: 0, gzip: 0 },
      "event-only": { raw: 63274, gzip: 21159 },
      "scalar-signal": { raw: 63282, gzip: 21160 },
      "show-for": { raw: 63428, gzip: 21246 },
      model: { raw: 63273, gzip: 21173 },
      "blog-theme": { raw: 63623, gzip: 21299 },
      "json-api": { raw: 63428, gzip: 21253 },
      "multi-island": { raw: 63550, gzip: 21263 },
    });
    const state = makeState(linuxContract.platform, linuxContract);
    expect(validate(state)).toEqual({ errors: [], passed: true });
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
