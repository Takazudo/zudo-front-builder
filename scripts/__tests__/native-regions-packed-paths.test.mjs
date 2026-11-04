import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { assertPackedInstallContainment } from "../../tests/router-chromium/packed-install-containment.mjs";

const workspaceFixture = fileURLToPath(
  new URL("../../tests/router-chromium/fixture/island-real.js", import.meta.url),
);
const roots = [];

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function makeWork() {
  const work = mkdtempSync(join(realpathSync(tmpdir()), "zfb-packed-paths-"));
  roots.push(work);
  return work;
}

function writePackage(dir, name, exportsMap) {
  mkdirSync(dir, { recursive: true });
  for (const rel of Object.values(exportsMap)) {
    const file = join(dir, rel);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, "export {};\n");
  }
  writeFileSync(
    join(dir, "package.json"),
    JSON.stringify({
      name,
      type: "module",
      exports: Object.fromEntries(
        Object.entries(exportsMap).map(([key, rel]) => [key, `./${rel}`]),
      ),
    }),
  );
  return dir;
}

function installPair(nodeModules) {
  writePackage(join(nodeModules, "@takazudo", "zfb"), "@takazudo/zfb", {
    "./runtime": "dist/runtime.js",
  });
  writePackage(join(nodeModules, "@takazudo", "zfb-runtime"), "@takazudo/zfb-runtime", {
    "./client-router": "dist/client-router/index.js",
  });
}

function resolveEntries(isolated) {
  const consumer = join(isolated, "consumer.mjs");
  writeFileSync(
    consumer,
    `export const sdk = import.meta.resolve("@takazudo/zfb/runtime");
export const runtime = import.meta.resolve("@takazudo/zfb-runtime/client-router");
`,
  );
  const env = { ...process.env, CONSUMER_URL: pathToFileURL(consumer).href };
  delete env.NODE_OPTIONS;
  const result = spawnSync(
    process.execPath,
    [
      "--input-type=module",
      "-e",
      `const api = await import(process.env.CONSUMER_URL);
process.stdout.write(JSON.stringify({ sdk: api.sdk, runtime: api.runtime }));
`,
    ],
    { encoding: "utf8", env },
  );
  if (result.status !== 0) {
    throw new Error(`import.meta.resolve failed (${result.status}): ${result.stderr}`);
  }
  return JSON.parse(result.stdout);
}

// The pre-repair checker compared dirname(fileURL) with this lexical prefix.
function lexicalNodeModulesPrefixHolds(isolated, sdkEntry, runtimeEntry) {
  const prefix = join(isolated, "node_modules");
  const sdkDist = dirname(fileURLToPath(sdkEntry));
  const runtimeDist = dirname(dirname(fileURLToPath(runtimeEntry)));
  return sdkDist.startsWith(prefix) && runtimeDist.startsWith(prefix);
}

function contained(isolated, entries) {
  return assertPackedInstallContainment({
    isolated,
    sdkEntry: entries.sdk,
    runtimeEntry: entries.runtime,
  });
}

describe("packed install containment", () => {
  it("accepts a physical temporary install resolved by Node", () => {
    const isolated = mkdtempSync(join(makeWork(), "zfb-native-regions-packed-"));
    expect(realpathSync(isolated)).toBe(isolated);
    installPair(join(isolated, "node_modules"));
    const entries = resolveEntries(isolated);

    expect(lexicalNodeModulesPrefixHolds(isolated, entries.sdk, entries.runtime)).toBe(true);
    const result = contained(isolated, entries);
    expect(result.isolatedRoot).toBe(isolated);
    expect(result.sdkPath.startsWith(`${result.nodeModulesRoot}${sep}`)).toBe(true);
    expect(result.runtimePath.startsWith(`${result.nodeModulesRoot}${sep}`)).toBe(true);
    expect(result.sdkDist).toBe(dirname(result.sdkPath));
    expect(result.runtimeDist).toBe(dirname(dirname(result.runtimePath)));
  });

  it("accepts a symlinked temporary ancestor that the lexical prefix rejects", () => {
    const work = makeWork();
    const realAncestor = join(work, "real-ancestor");
    const aliasAncestor = join(work, "alias-ancestor");
    mkdirSync(realAncestor);
    symlinkSync(realAncestor, aliasAncestor, "dir");
    const isolated = mkdtempSync(join(aliasAncestor, "zfb-native-regions-packed-"));
    expect(isolated.includes(`${sep}alias-ancestor${sep}`)).toBe(true);
    expect(realpathSync(isolated).includes(`${sep}alias-ancestor${sep}`)).toBe(false);
    installPair(join(isolated, "node_modules"));
    const entries = resolveEntries(isolated);

    const sdkDist = dirname(fileURLToPath(entries.sdk));
    const canonicalPrefix = realpathSync(join(isolated, "node_modules"));
    expect(lexicalNodeModulesPrefixHolds(isolated, entries.sdk, entries.runtime)).toBe(false);
    expect(sdkDist.startsWith(`${canonicalPrefix}${sep}`)).toBe(true);

    const result = contained(isolated, entries);
    expect(result.isolatedRoot).toBe(realpathSync(isolated));
    expect(result.isolatedRoot).not.toBe(isolated);
    expect(result.nodeModulesRoot).toBe(canonicalPrefix);
    expect(result.sdkPath.startsWith(`${result.nodeModulesRoot}${sep}`)).toBe(true);
    expect(result.runtimePath.startsWith(`${result.nodeModulesRoot}${sep}`)).toBe(true);
  });

  it("accepts a package symlink that stays inside the isolated node_modules", () => {
    const isolated = mkdtempSync(join(makeWork(), "zfb-native-regions-packed-"));
    const nodeModules = join(isolated, "node_modules");
    const stored = join(nodeModules, ".store", "zfb");
    writePackage(stored, "@takazudo/zfb", { "./runtime": "dist/runtime.js" });
    mkdirSync(join(nodeModules, "@takazudo"), { recursive: true });
    symlinkSync(stored, join(nodeModules, "@takazudo", "zfb"), "dir");
    writePackage(join(nodeModules, "@takazudo", "zfb-runtime"), "@takazudo/zfb-runtime", {
      "./client-router": "dist/client-router/index.js",
    });
    const entries = resolveEntries(isolated);
    const result = contained(isolated, entries);
    expect(result.sdkPath.startsWith(`${result.nodeModulesRoot}${sep}.store${sep}`)).toBe(true);
  });

  it("rejects a node_modules-other sibling that satisfies the lexical prefix", () => {
    const isolated = mkdtempSync(join(makeWork(), "zfb-native-regions-packed-"));
    const nodeModules = join(isolated, "node_modules");
    writePackage(join(nodeModules, "@takazudo", "zfb-runtime"), "@takazudo/zfb-runtime", {
      "./client-router": "dist/client-router/index.js",
    });
    const siblingSdk = join(
      isolated,
      "node_modules-other",
      "@takazudo",
      "zfb",
      "dist",
      "runtime.js",
    );
    mkdirSync(dirname(siblingSdk), { recursive: true });
    writeFileSync(siblingSdk, "export {};\n");
    const sdkEntry = pathToFileURL(siblingSdk).href;
    const runtimeEntry = pathToFileURL(
      join(nodeModules, "@takazudo", "zfb-runtime", "dist", "client-router", "index.js"),
    ).href;

    expect(dirname(fileURLToPath(sdkEntry)).startsWith(join(isolated, "node_modules"))).toBe(true);
    expect(() => assertPackedInstallContainment({ isolated, sdkEntry, runtimeEntry })).toThrow(
      /sdk entry escapes the isolated install/,
    );
  });

  it("rejects a package symlink that resolves outside the install", () => {
    const work = makeWork();
    const outside = join(work, "outside-sdk");
    writePackage(outside, "@takazudo/zfb", { "./runtime": "dist/runtime.js" });
    const isolated = mkdtempSync(join(work, "zfb-native-regions-packed-"));
    const nodeModules = join(isolated, "node_modules");
    mkdirSync(join(nodeModules, "@takazudo"), { recursive: true });
    symlinkSync(outside, join(nodeModules, "@takazudo", "zfb"), "dir");
    writePackage(join(nodeModules, "@takazudo", "zfb-runtime"), "@takazudo/zfb-runtime", {
      "./client-router": "dist/client-router/index.js",
    });
    const entries = resolveEntries(isolated);

    expect(entries.sdk.endsWith("/dist/runtime.js")).toBe(true);
    expect(realpathSync(fileURLToPath(entries.sdk))).toBe(
      realpathSync(join(outside, "dist", "runtime.js")),
    );
    expect(() => contained(isolated, entries)).toThrow(/sdk entry escapes the isolated install/);
  });

  it("rejects an entry symlink to a workspace fixture", () => {
    const isolated = mkdtempSync(join(makeWork(), "zfb-native-regions-packed-"));
    const nodeModules = join(isolated, "node_modules");
    installPair(nodeModules);
    const sdkLink = join(nodeModules, "@takazudo", "zfb", "dist", "runtime.js");
    rmSync(sdkLink);
    symlinkSync(workspaceFixture, sdkLink);
    const sdkEntry = pathToFileURL(sdkLink).href;
    const runtimeEntry = pathToFileURL(
      join(nodeModules, "@takazudo", "zfb-runtime", "dist", "client-router", "index.js"),
    ).href;

    expect(sdkEntry.endsWith("/dist/runtime.js")).toBe(true);
    expect(() => assertPackedInstallContainment({ isolated, sdkEntry, runtimeEntry })).toThrow(
      new RegExp(
        `sdk entry escapes the isolated install[\\s\\S]*${realpathSync(workspaceFixture).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`,
      ),
    );
  });

  it("rejects a node_modules symlink that escapes the isolated root", () => {
    const work = makeWork();
    const outside = join(work, "outside-node-modules");
    installPair(outside);
    const isolated = mkdtempSync(join(work, "zfb-native-regions-packed-"));
    symlinkSync(outside, join(isolated, "node_modules"), "dir");
    const entries = resolveEntries(isolated);

    expect(entries.sdk.endsWith("/dist/runtime.js")).toBe(true);
    expect(entries.runtime.endsWith("/dist/client-router/index.js")).toBe(true);
    expect(
      realpathSync(fileURLToPath(entries.sdk)).startsWith(`${realpathSync(outside)}${sep}`),
    ).toBe(true);
    expect(() => contained(isolated, entries)).toThrow(/node_modules escapes the isolated install/);
  });

  it("fails closed on a missing entry and an unexpected dist shape", () => {
    const isolated = mkdtempSync(join(makeWork(), "zfb-native-regions-packed-"));
    const nodeModules = join(isolated, "node_modules");
    installPair(nodeModules);
    const runtimeEntry = pathToFileURL(
      join(nodeModules, "@takazudo", "zfb-runtime", "dist", "client-router", "index.js"),
    ).href;
    const missingSdk = pathToFileURL(
      join(nodeModules, "@takazudo", "zfb", "dist", "runtime.js"),
    ).href;
    rmSync(fileURLToPath(missingSdk));

    expect(() =>
      assertPackedInstallContainment({ isolated, sdkEntry: missingSdk, runtimeEntry }),
    ).toThrow(/sdk entry file is missing/);

    const wrongSdk = pathToFileURL(join(nodeModules, "@takazudo", "zfb", "dist", "other.js")).href;
    writeFileSync(fileURLToPath(wrongSdk), "export {};\n");
    expect(() =>
      assertPackedInstallContainment({ isolated, sdkEntry: wrongSdk, runtimeEntry }),
    ).toThrow(/unexpected sdk entry shape/);

    const sdkEntry = pathToFileURL(
      join(nodeModules, "@takazudo", "zfb", "dist", "runtime.js"),
    ).href;
    writeFileSync(fileURLToPath(sdkEntry), "export {};\n");
    const wrongRuntime = pathToFileURL(
      join(nodeModules, "@takazudo", "zfb-runtime", "dist", "client-router", "router.js"),
    ).href;
    expect(() =>
      assertPackedInstallContainment({ isolated, sdkEntry, runtimeEntry: wrongRuntime }),
    ).toThrow(/unexpected runtime entry shape/);

    expect(() =>
      assertPackedInstallContainment({
        isolated: "relative/install",
        sdkEntry,
        runtimeEntry,
      }),
    ).toThrow(/isolated install root must be an absolute path/);
  });
});
