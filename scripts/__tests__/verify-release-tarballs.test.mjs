// scripts/release/verify-release-tarballs.mjs — the publish job's independent
// re-inspection of the pack job's tarballs (#4690). Every attack below keeps
// manifest.json self-consistent (integrity recomputed), as a compromised pack
// job could; only the content checks against publish's own inputs catch it.
import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { main, verifyTarballs } from "../release/verify-release-tarballs.mjs";
import { cleanupTmp, tmp } from "./helpers/darwin-fixture.mjs";
import {
  V,
  integrityOf,
  makeTarballSet,
  replaceTarball,
  verdictFor,
  wasmFiles,
} from "./helpers/release-tarballs-fixture.mjs";

afterEach(cleanupTmp);
const sets = [];
afterEach(() => {
  for (const set of sets.splice(0)) rmSync(set.base, { recursive: true, force: true });
});
const fresh = () => {
  const set = makeTarballSet({ base: tmp("zfb-verify-tgz-") });
  sets.push(set);
  return set;
};
const verify = (set, overrides = {}) =>
  verifyTarballs({
    dir: set.out,
    version: V,
    verdict: verdictFor(),
    mdWasmManifest: wasmFiles().manifest,
    ...overrides,
  });
const pkgJson = (obj) => ({
  name: "package/package.json",
  data: Buffer.from(JSON.stringify(obj)),
  mode: 0o644,
});

describe("verify-release-tarballs", () => {
  it("accepts an untouched pack output", () => {
    expect(verify(fresh())).toEqual([]);
  });

  it("rejects a tampered Darwin binary even with a self-consistent manifest", () => {
    const set = fresh();
    replaceTarball(set.out, "@takazudo/zfb-darwin-x64", [
      pkgJson({ name: "@takazudo/zfb-darwin-x64", version: V }),
      { name: "package/zfb", data: Buffer.from("evil binary"), mode: 0o755 },
      {
        name: "package/zfb-css.node",
        data: Buffer.from("css @takazudo/zfb-darwin-x64"),
        mode: 0o644,
      },
    ]);
    expect(verify(set).join("\n")).toMatch(
      /@takazudo\/zfb-darwin-x64: package\/zfb sha256 .* != admitted/,
    );
  });

  it("rejects a Darwin carrier relabeled as a Linux one to dodge the verdict", () => {
    const set = fresh();
    const manifestPath = join(set.out, "manifest.json");
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    manifest.packages.find((p) => p.name === "@takazudo/zfb-darwin-x64").dir =
      "packages/zfb-linux-x64-gnu";
    writeFileSync(manifestPath, JSON.stringify(manifest));
    expect(verify(set).join("\n")).toContain("does not belong to this package");
  });

  it("requires an admitted verdict for Darwin carriers", () => {
    expect(verify(fresh(), { verdict: null }).join("\n")).toContain("no admitted Darwin verdict");
  });

  it("rejects a packed name/version that differs from the manifest row or release version", () => {
    const set = fresh();
    replaceTarball(set.out, "create-zfb", [pkgJson({ name: "create-zfb", version: "9.9.9" })]);
    expect(verify(set).join("\n")).toContain("packed package.json is create-zfb@9.9.9");
    expect(verify(fresh(), { version: "6.1.1" }).join("\n")).toContain(
      "manifest version 6.1.0 is not 6.1.1",
    );
  });

  it("rejects a non-Windows carrier whose zfb is not 0755", () => {
    const set = fresh();
    replaceTarball(set.out, "@takazudo/zfb-linux-x64-gnu", [
      pkgJson({ name: "@takazudo/zfb-linux-x64-gnu", version: V }),
      { name: "package/zfb", data: Buffer.from("bin"), mode: 0o644 },
      { name: "package/zfb-css.node", data: Buffer.from("css"), mode: 0o644 },
    ]);
    expect(verify(set).join("\n")).toContain("binary zfb has mode 644");
  });

  it("rejects a carrier without zfb-css.node while @takazudo/zfb exports ./css", () => {
    const set = fresh();
    replaceTarball(set.out, "@takazudo/zfb-win32-x64-msvc", [
      pkgJson({ name: "@takazudo/zfb-win32-x64-msvc", version: V }),
      { name: "package/zfb.exe", data: Buffer.from("exe"), mode: 0o644 },
    ]);
    expect(verify(set).join("\n")).toContain(
      "@takazudo/zfb-win32-x64-msvc: zfb-css.node is missing",
    );
  });

  it("rejects a leftover workspace: specifier", () => {
    const set = fresh();
    replaceTarball(set.out, "@takazudo/zfb-runtime", [
      pkgJson({
        name: "@takazudo/zfb-runtime",
        version: V,
        dependencies: { "@takazudo/zfb": "workspace:^" },
      }),
    ]);
    expect(verify(set).join("\n")).toContain("dependencies.@takazudo/zfb is still workspace:^");
  });

  it("checks MD/WASM against its packed manifest and build-wasm-md's manifest", () => {
    expect(verify(fresh(), { mdWasmManifest: Buffer.from("{}") }).join("\n")).toContain(
      "differs from the build-wasm-md manifest",
    );
    expect(verify(fresh(), { mdWasmManifest: null }).join("\n")).toContain(
      "no build-wasm-md manifest",
    );
    expect(verify(fresh(), { mdWasmManifest: null, allowMissingMdWasmManifest: true })).toEqual([]);
  });

  it("rejects a tarball whose bytes no longer match the manifest integrity", () => {
    const set = fresh();
    const row = set.manifest.packages[0];
    writeFileSync(join(set.out, row.file), Buffer.from("tampered"));
    expect(verify(set).join("\n")).toContain("tarball sha512 differs from the manifest");
    expect(integrityOf(Buffer.from("tampered"))).not.toBe(row.integrity);
  });

  it("CLI exits 0 / 1 / 2", () => {
    const set = fresh();
    const sink = { write: () => {} };
    const args = [
      "--dir",
      set.out,
      "--version",
      V,
      "--admission",
      set.verdictPath,
      "--md-wasm-manifest",
      set.buildManifestPath,
    ];
    expect(main(args, { out: sink, err: sink })).toBe(0);
    expect(main(["--dir", set.out, "--version", V], { out: sink, err: sink })).toBe(1);
    expect(main(["--dir", set.out], { out: sink, err: sink })).toBe(2);
  });
});
