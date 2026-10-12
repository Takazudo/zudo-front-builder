import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vite-plus/test";

import {
  NORMALIZED_SIDECAR,
  normalizeFixtureFiles,
  normalizeMeasurementDirectory,
  normalizerSha256,
} from "../../research/v3-island-size/normalize-identity.mjs";
import { assertLinuxX64Provenance, measuredTotals } from "../rebaseline-island-size-linux.mjs";
import {
  buildEvidence,
  checkMigrationTotals,
  darwinReadmeSection,
  loadDarwinArtifact,
  migrateDarwinContract,
  parseArgs,
  validateDarwinEvidence,
} from "../rebaseline-island-size-darwin.mjs";

const testDir = dirname(fileURLToPath(import.meta.url));
const legacy = JSON.parse(
  readFileSync(join(testDir, "fixtures/island-size-darwin-actual-gzip-v1.json"), "utf8"),
);
const SOURCE_SHA = "a".repeat(40);
const VERSION = "9.9.9";
const MODES = ["workspace", "packed"];
const FIXTURES = Object.keys(legacy.ceilings.workspace);
const roots = [];

function sha(value) {
  return createHash("sha256").update(value).digest("hex");
}

function filesFor(mode, fixture) {
  const token = sha(mode + fixture).slice(0, 16);
  if (fixture === "no-island") {
    return { "index.html": Buffer.from("<!doctype html><body>empty</body>\n") };
  }
  const entry =
    "// " +
    fixture +
    '\nfunction r(n){return{identity:{component:n,build:"' +
    token +
    '"},mount:()=>{let l={component:n,build:"' +
    token +
    '"};return l}}}\nexport {};\n';
  return {
    "index.html": Buffer.from(
      '<!doctype html><script type="module" src="/assets/islands-x.js"></script><div data-zfb-build="' +
        token +
        '"></div>\n',
    ),
    "assets/islands-x.js": Buffer.from(entry),
  };
}

function write(root, relative, bytes) {
  const path = join(root, relative);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, bytes);
}

// Builds a complete synthetic native artifact. `tweak` mutates provenance
// before the sidecars are written so the on-disk run stays self-consistent.
function makeArtifact({ platform = { os: "darwin", arch: "arm64" }, tweak } = {}) {
  const root = mkdtempSync(join(tmpdir(), "zfb-darwin-migration-"));
  roots.push(root);
  const digests = {
    zfbBinarySha256: sha("zfb-binary"),
    esbuildBinarySha256: sha("esbuild-binary"),
    zfbTarballSha256: sha("zfb-tgz"),
    runtimeTarballSha256: sha("runtime-tgz"),
    slugifyTarballSha256: sha("slugify-tgz"),
  };
  write(root, "pack/takazudo-zfb-" + VERSION + ".tgz", "zfb-tgz");
  write(root, "pack/takazudo-zfb-runtime-" + VERSION + ".tgz", "runtime-tgz");
  write(root, "pack/takazudo-zfb-slugify-" + VERSION + ".tgz", "slugify-tgz");
  write(root, "packed-package/package/package.json", '{"name":"@takazudo/zfb"}');
  write(root, "packed-runtime/package/package.json", '{"name":"@takazudo/zfb-runtime"}');
  write(root, "packed-slugify/package/package.json", '{"name":"@takazudo/zfb-slugify"}');
  write(root, "check-budget.exit-code", "1\n");
  write(root, "check-budget.log", "Island size budget rejected:\n");
  write(
    root,
    "runner-provenance.txt",
    [
      "source_sha=" + SOURCE_SHA,
      "runner=Darwin arm64",
      "sw_vers=ProductName:\t\tmacOS;ProductVersion:\t\t15.7.9;BuildVersion:\t\t24G830;",
      "rustc=rustc 1.99.0 (b940084d7 2026-09-28);host: aarch64-apple-darwin;",
      "cargo=cargo 1.99.0 (5f94df478 2026-08-27)",
      "node=" + legacy.toolchain.nodeVersion,
      "zlib=" + legacy.toolchain.zlibVersion,
      "pnpm=12.8.2",
      "zfb_binary=" + digests.zfbBinarySha256 + "  target/debug/zfb",
      "esbuild_version=" + legacy.toolchain.esbuildVersion,
      "esbuild_binary=" + digests.esbuildBinarySha256 + "  esbuild",
      "hono_version=" + legacy.toolchain.honoVersion,
      "slugify_tarball=" + digests.slugifyTarballSha256 + "  x.tgz",
      "zfb_tarball=" + digests.zfbTarballSha256 + "  y.tgz",
      "runtime_tarball=" + digests.runtimeTarballSha256 + "  z.tgz",
      "",
    ].join("\n"),
  );
  for (const mode of MODES) {
    const results = {};
    for (const fixture of FIXTURES) {
      const files = filesFor(mode, fixture);
      for (const pass of [1, 2]) {
        for (const [name, bytes] of Object.entries(files)) {
          write(root, "real-" + mode + "/" + fixture + "/pass-" + pass + "/" + name, bytes);
        }
      }
      const actual = normalizeFixtureFiles(files, mode + " " + fixture);
      results[fixture] = {
        chunks: actual.chunks.map(({ file, raw, gzip, sha256, phase, role }) => ({
          file,
          raw,
          gzip,
          sha256,
          phase,
          role,
        })),
        initial: { raw: actual.initial.raw, gzip: actual.initial.gzip },
        later: { raw: actual.later.raw, gzip: actual.later.gzip },
        shipped: { raw: actual.shipped.raw, gzip: actual.shipped.gzip },
      };
    }
    const provenance = {
      sourceSha: SOURCE_SHA,
      mode,
      platform,
      fixtureVersion: legacy.fixtureVersion,
      fixtureSha256: legacy.fixtureSha256,
      runnerSha256: legacy.runnerSha256,
      packageVersion: VERSION,
      runtimePackageVersion: VERSION,
      packageJsonSha256: sha('{"name":"@takazudo/zfb"}'),
      runtimePackageJsonSha256: sha('{"name":"@takazudo/zfb-runtime"}'),
      slugifyPackageJsonSha256: sha('{"name":"@takazudo/zfb-slugify"}'),
      honoPackageVersion: legacy.toolchain.honoVersion,
      esbuildVersion: legacy.toolchain.esbuildVersion,
      nodeVersion: legacy.toolchain.nodeVersion,
      zlibVersion: legacy.toolchain.zlibVersion,
      compression: legacy.compression,
      repeats: 2,
      pnpmLockSha256: sha("pnpm-lock"),
      cargoLockSha256: sha("cargo-lock"),
      zfbBinarySha256: digests.zfbBinarySha256,
      esbuildBinarySha256: digests.esbuildBinarySha256,
      zfbTarballSha256: digests.zfbTarballSha256,
      runtimeTarballSha256: digests.runtimeTarballSha256,
    };
    tweak?.(provenance, mode);
    const path = join(root, "real-" + mode, "measurement.json");
    writeFileSync(path, JSON.stringify({ provenance, results }, null, 2) + "\n");
    const { sidecar } = normalizeMeasurementDirectory(join(root, "real-" + mode));
    writeFileSync(
      join(root, "real-" + mode, NORMALIZED_SIDECAR),
      JSON.stringify(sidecar, null, 2) + "\n",
    );
  }
  return root;
}

// A schemaVersion-1 contract whose raw ceilings equal the synthetic artifact
// and whose actual-gzip ceilings are `gzipSlack` above the normalized totals.
function contractFor(root, gzipSlack = 2) {
  const contract = structuredClone(legacy);
  for (const mode of MODES) {
    const { sidecar } = normalizeMeasurementDirectory(join(root, "real-" + mode));
    for (const fixture of FIXTURES) {
      const { raw, gzipNormalized } = sidecar.results[fixture].shipped;
      contract.ceilings[mode][fixture] = {
        raw,
        gzip: gzipNormalized === 0 ? 0 : gzipNormalized + gzipSlack,
      };
    }
  }
  return contract;
}

const args = {
  artifact: "unused",
  "run-id": "7",
  "job-id": "8",
  "artifact-id": "9",
  "artifact-expires-at": "2026-11-11T07:26:59Z",
  "source-sha": SOURCE_SHA,
  reason: "metric-migration",
};

function totalsOf(artifact) {
  return measuredTotals({
    workspace: artifact.modes.workspace.recomputed.results,
    packed: artifact.modes.packed.recomputed.results,
  });
}

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("rebaseline-island-size-darwin", () => {
  it("migrates a complete native artifact using the artifact's own provenance", () => {
    const root = makeArtifact();
    const contract = contractFor(root);
    const artifact = loadDarwinArtifact(root);
    expect(validateDarwinEvidence(contract, artifact, SOURCE_SHA)).toEqual([]);
    const next = migrateDarwinContract(contract, totalsOf(artifact), buildEvidence(args, artifact));
    expect(next.schemaVersion).toBe(2);
    expect(next.metric).toBe("gzip-identity-normalized");
    expect(next.normalizationVersion).toBe(1);
    expect(next.normalizerSha256).toBe(normalizerSha256());
    expect(next.allowance).toEqual({ raw: 0, gzipNormalized: 0 });
    expect(next.platform).toEqual({ os: "darwin", arch: "arm64" });
    expect(next.baselineSourceSha).toBe(SOURCE_SHA);
    expect(next.toolchain.packageVersion).toBe(VERSION);
    expect(next.toolchain.pnpmLockSha256).toBe(sha("pnpm-lock"));
    expect(next.toolchain.cargoLockSha256).toBe(sha("cargo-lock"));
    expect(Object.keys(next).slice(0, 4)).toEqual([
      "schemaVersion",
      "metric",
      "normalizationVersion",
      "normalizerSha256",
    ]);
    for (const mode of MODES) {
      for (const fixture of FIXTURES) {
        expect(Object.keys(next.ceilings[mode][fixture])).toEqual(["raw", "gzipNormalized"]);
        expect(next.ceilings[mode][fixture].raw).toBe(contract.ceilings[mode][fixture].raw);
        expect(next.baselineEvidence.actualGzip[mode][fixture]).toBe(
          artifact.modes[mode].recomputed.results[fixture].shipped.gzip,
        );
      }
    }
    expect(next.baselineEvidence).toMatchObject({
      event: "ci-artifact-replay",
      runId: 7,
      jobId: 8,
      artifact: { id: 9, expiresAt: "2026-11-11T07:26:59Z" },
      nativeRunner: { platform: "darwin-arm64", macOS: "15.7.9 (24G830)" },
      migratedFrom: { schemaVersion: 1, metric: "gzip" },
      originalCheckerResult: { exitCode: 1 },
    });
    expect(next.platformStatus).toContain("not a native measurement of the current source");
    expect(next.platformStatus).toContain("#4170");
    const readme = darwinReadmeSection(next, contract);
    expect(readme).toContain("Original semantics");
    expect(readme).toContain("historical-artifact evidence");
    expect(readme).toContain("#4170");
  });

  it("accepts only the metric-migration reason and validates arguments", () => {
    expect(parseArgs(Object.entries(args).flatMap(([k, v]) => ["--" + k, v]))["run-id"]).toBe("7");
    const bad = (override) => () =>
      parseArgs(Object.entries({ ...args, ...override }).flatMap(([k, v]) => ["--" + k, v]));
    expect(bad({ reason: "release-bump" })).toThrow("only the metric migration");
    expect(bad({ reason: "runtime-identity" })).toThrow("--reason must be metric-migration");
    expect(bad({ "source-sha": "abc" })).toThrow("40-character");
    expect(bad({ "run-id": "x" })).toThrow("positive integer");
    expect(bad({ "artifact-expires-at": "never" })).toThrow("ISO timestamp");
    expect(() => parseArgs(["--artifact", "x"])).toThrow("required --run-id");
  });

  it("rejects a Linux or unknown platform on either side", () => {
    const linuxRoot = makeArtifact({ platform: { os: "linux", arch: "x64" } });
    const linuxArtifact = loadDarwinArtifact(linuxRoot);
    expect(
      validateDarwinEvidence(contractFor(linuxRoot), linuxArtifact, SOURCE_SHA).join("\n"),
    ).toContain("measured platform is not darwin-arm64");
    const root = makeArtifact();
    const artifact = loadDarwinArtifact(root);
    const linuxContract = { ...contractFor(root), platform: { os: "linux", arch: "x64" } };
    expect(validateDarwinEvidence(linuxContract, artifact, SOURCE_SHA)).toContain(
      "contract is not the Darwin arm64 contract",
    );
    const migrated = migrateDarwinContract(
      contractFor(root),
      totalsOf(artifact),
      buildEvidence(args, artifact),
    );
    expect(validateDarwinEvidence(migrated, artifact, SOURCE_SHA)).toContain(
      "contract is not a schemaVersion 1 actual-gzip contract",
    );
    expect(() => assertLinuxX64Provenance({ platform: { os: "darwin", arch: "arm64" } })).toThrow(
      "not linux-x64",
    );
    expect(() =>
      assertLinuxX64Provenance({ platform: { os: "linux", arch: "x64" } }),
    ).not.toThrow();
  });

  it("rejects a missing normalized sidecar", () => {
    const root = makeArtifact();
    rmSync(join(root, "real-packed", NORMALIZED_SIDECAR));
    expect(() => loadDarwinArtifact(root)).toThrow(
      "missing measurement.normalized.json for packed",
    );
  });

  it("rejects a stale sidecar that disagrees with the saved bytes", () => {
    const root = makeArtifact();
    const path = join(root, "real-workspace", NORMALIZED_SIDECAR);
    const sidecar = JSON.parse(readFileSync(path, "utf8"));
    sidecar.results["event-only"].shipped.gzipNormalized -= 5;
    writeFileSync(path, JSON.stringify(sidecar));
    const errors = validateDarwinEvidence(contractFor(root), loadDarwinArtifact(root), SOURCE_SHA);
    expect(errors).toContain(
      "workspace: sidecar disagrees with the recomputation from the saved bytes",
    );
  });

  it("rejects a token shape or site count the normalizer does not recognize", () => {
    const mismatched = makeArtifact();
    for (const pass of [1, 2]) {
      const html = join(mismatched, "real-workspace/event-only/pass-" + pass + "/index.html");
      writeFileSync(
        html,
        readFileSync(html, "utf8").replace(
          /data-zfb-build="[0-9a-f]+"/,
          'data-zfb-build="0123456789abcdef"',
        ),
      );
    }
    expect(() => loadDarwinArtifact(mismatched)).toThrow("build identity site carries");

    const malformed = makeArtifact();
    for (const pass of [1, 2]) {
      const html = join(malformed, "real-packed/model/pass-" + pass + "/index.html");
      writeFileSync(
        html,
        readFileSync(html, "utf8").replace(/data-zfb-build="[0-9a-f]+"/, 'data-zfb-build="XYZ"'),
      );
    }
    expect(() => loadDarwinArtifact(malformed)).toThrow("16 lowercase hex");
  });

  it("rejects saved bytes that disagree with the claimed measurement or between passes", () => {
    const tampered = makeArtifact();
    const js = join(tampered, "real-workspace/event-only/pass-1/assets/islands-x.js");
    writeFileSync(js, readFileSync(js, "utf8") + "// extra\n");
    expect(() => loadDarwinArtifact(tampered)).toThrow(/saved repeated dist inventories differ/);

    const passEqual = makeArtifact();
    for (const pass of [1, 2]) {
      const file = join(
        passEqual,
        "real-workspace/event-only/pass-" + pass + "/assets/islands-x.js",
      );
      writeFileSync(file, readFileSync(file, "utf8") + "// extra\n");
    }
    expect(() => loadDarwinArtifact(passEqual)).toThrow(/measurement\.json claims raw/);
  });

  it("rejects inconsistent provenance, packages and runner files", () => {
    const root = makeArtifact();
    writeFileSync(join(root, "pack", "takazudo-zfb-" + VERSION + ".tgz"), "different bytes");
    writeFileSync(join(root, "packed-runtime/package/package.json"), "{}");
    const errors = validateDarwinEvidence(
      contractFor(root),
      loadDarwinArtifact(root),
      SOURCE_SHA,
    ).join("\n");
    expect(errors).toContain("saved zfb tarball is missing or differs");
    expect(errors).toContain("saved packed runtime package.json is missing or differs");
    const runnerPath = join(root, "runner-provenance.txt");
    writeFileSync(
      runnerPath,
      readFileSync(runnerPath, "utf8")
        .replace("runner=Darwin arm64", "runner=Linux x86_64")
        .replace(/zfb_tarball=[0-9a-f]+/, "zfb_tarball=" + "0".repeat(64)),
    );
    const runnerErrors = validateDarwinEvidence(
      contractFor(root),
      loadDarwinArtifact(root),
      SOURCE_SHA,
    ).join("\n");
    expect(runnerErrors).toContain("runner-provenance.txt runner is missing or inconsistent");
    expect(runnerErrors).toContain("runner-provenance.txt zfb_tarball is missing or inconsistent");

    const lockDrift = makeArtifact({
      tweak: (p, mode) => {
        if (mode === "packed") p.cargoLockSha256 = sha("other");
      },
    });
    expect(
      validateDarwinEvidence(contractFor(lockDrift), loadDarwinArtifact(lockDrift), SOURCE_SHA),
    ).toContain("packed: cargoLockSha256 differs between workspace and packed");

    const wrongSource = makeArtifact();
    expect(
      validateDarwinEvidence(
        contractFor(wrongSource),
        loadDarwinArtifact(wrongSource),
        "b".repeat(40),
      ).join("\n"),
    ).toContain("source SHA " + SOURCE_SHA);

    const wrongToolchain = makeArtifact({ tweak: (p) => (p.nodeVersion = "v22.0.0") });
    expect(
      validateDarwinEvidence(
        contractFor(wrongToolchain),
        loadDarwinArtifact(wrongToolchain),
        SOURCE_SHA,
      ),
    ).toContain("workspace: nodeVersion differs from the contract");
  });

  it("refuses changed raw totals, a missing fixture and any size increase", () => {
    const root = makeArtifact();
    const artifact = loadDarwinArtifact(root);
    const totals = totalsOf(artifact);

    const rawMoved = contractFor(root);
    rawMoved.ceilings.packed.model.raw -= 1;
    expect(() => migrateDarwinContract(rawMoved, totals, buildEvidence(args, artifact))).toThrow(
      "raw totals changed (packed/model)",
    );

    const missing = contractFor(root);
    delete missing.ceilings.workspace["json-api"];
    expect(checkMigrationTotals(missing, totals).join("\n")).toContain(
      "fixture set differs from the contract (workspace/json-api)",
    );

    const grown = contractFor(root);
    grown.ceilings.workspace["show-for"].gzip = totals.workspace["show-for"].gzipNormalized - 1;
    expect(() => migrateDarwinContract(grown, totals, buildEvidence(args, artifact))).toThrow(
      "that is a size increase, not a metric migration",
    );

    const inflated = structuredClone(totals);
    inflated.packed.model.gzipNormalized = inflated.packed.model.gzip + 1;
    expect(checkMigrationTotals(contractFor(root, 50), inflated).join("\n")).toContain(
      "packed/model: gzipNormalized exceeds actual gzip",
    );
  });

  it("keeps non-identity product bytes visible while ignoring only the token", () => {
    const files = filesFor("workspace", "event-only");
    const base = normalizeFixtureFiles(files, "base");
    const retoken = { ...files };
    const oldToken = /data-zfb-build="([0-9a-f]{16})"/.exec(files["index.html"].toString())[1];
    const swap = (buffer) => Buffer.from(buffer.toString().replaceAll(oldToken, "f".repeat(16)));
    retoken["index.html"] = swap(files["index.html"]);
    retoken["assets/islands-x.js"] = swap(files["assets/islands-x.js"]);
    const moved = normalizeFixtureFiles(retoken, "moved");
    expect(moved.shipped.gzipNormalized).toBe(base.shipped.gzipNormalized);
    expect(moved.chunks[0].sha256Normalized).toBe(base.chunks[0].sha256Normalized);
    expect(moved.chunks[0].sha256).not.toBe(base.chunks[0].sha256);

    const edited = { ...files };
    edited["assets/islands-x.js"] = Buffer.from(
      files["assets/islands-x.js"].toString().replace("export {};", "export {};/*x*/"),
    );
    const changed = normalizeFixtureFiles(edited, "edited");
    expect(changed.shipped.raw).toBeGreaterThan(base.shipped.raw);
    expect(changed.chunks[0].sha256Normalized).not.toBe(base.chunks[0].sha256Normalized);
  });
});
