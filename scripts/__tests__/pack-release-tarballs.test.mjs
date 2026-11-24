// scripts/release/pack-release-tarballs.mjs — the pack job (#4690). npm/pnpm
// are an injected fake that writes real gzip tarballs, so the packed-byte
// checks (0755, workspace:, Darwin digests, MD/WASM manifest) run for real.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { parseTarGz } from "../admit-darwin-artifacts.mjs";
import * as npmTarball from "../release/npm-tarball-members.mjs";
import { policyForTree } from "../release/npm-publication-policy.mjs";
import { MD_WASM_PATHS } from "../verify-zfb-md-wasm-published-digests.mjs";
import {
  PLATFORM,
  WORKSPACE,
  inventory,
  main,
  packRelease,
  parseLocalBuilt,
  provenanceFor,
} from "../release/pack-release-tarballs.mjs";
import { commitFixture } from "./helpers/release-tarballs-fixture.mjs";
import { cleanupTmp, makeTarGz, tmp } from "./helpers/darwin-fixture.mjs";

const V = "6.1.0";
const sha256 = (data) => createHash("sha256").update(Buffer.from(data)).digest("hex");
afterEach(cleanupTmp);

const NAMES = {
  "packages/zfb-darwin-arm64": "@takazudo/zfb-darwin-arm64",
  "packages/zfb-darwin-x64": "@takazudo/zfb-darwin-x64",
  "packages/zfb-linux-arm64-gnu": "@takazudo/zfb-linux-arm64-gnu",
  "packages/zfb-linux-x64-gnu": "@takazudo/zfb-linux-x64-gnu",
  "packages/zfb-win32-x64-msvc": "@takazudo/zfb-win32-x64-msvc",
  "packages/zfb-slugify": "@takazudo/zfb-slugify",
  "packages/zfb": "@takazudo/zfb",
  "packages/zfb-runtime": "@takazudo/zfb-runtime",
  "packages/zfb-adapter-cloudflare": "@takazudo/zfb-adapter-cloudflare",
  "packages/create-zfb": "create-zfb",
  "crates/zfb-md-wasm/npm": "@takazudo/zfb-md-wasm",
};

function wasmFiles(version = V) {
  const files = {};
  const artifacts = MD_WASM_PATHS.map(([entry, path], index) => {
    const data = Buffer.from(`wasm-${index}`);
    files[path] = data;
    return { entry, path, bytes: data.length, sha256: sha256(data) };
  });
  const manifest = Buffer.from(
    JSON.stringify({ schemaVersion: 1, name: "@takazudo/zfb-md-wasm", version, artifacts }),
  );
  return { files, manifest };
}

/** A package tree like release.yml's pack job sees after placing binaries. */
function tree({ omit = [], css = true } = {}) {
  const root = tmp("zfb-pack-root-");
  for (const [dir, name] of Object.entries(NAMES)) {
    if (omit.includes(dir)) continue;
    mkdirSync(join(root, dir), { recursive: true });
    const pkg = { name, version: V };
    if (dir === "packages/zfb") {
      pkg.dependencies = { "@takazudo/zfb-slugify": "workspace:*" };
      if (css) pkg.publishConfig = { exports: { "./css": "./css.js" } };
    }
    if (["packages/zfb", "packages/zfb-runtime"].includes(dir)) {
      pkg.scripts = { prepublishOnly: "pnpm build && pnpm test" };
    }
    writeFileSync(join(root, dir, "package.json"), JSON.stringify(pkg));
    if (PLATFORM[dir]) {
      writeFileSync(join(root, dir, PLATFORM[dir].binary), `bin ${name}`);
      if (css) writeFileSync(join(root, dir, "zfb-css.node"), `css ${name}`);
    }
  }
  return root;
}

/** Fake npm/pnpm: `pack` writes a real tarball of the dir into the destination. */
function fakeRun(root, { binaryMode = 0o755, keepWorkspace = false, wasm = wasmFiles() } = {}) {
  const calls = [];
  const run = (command, args, { cwd }) => {
    calls.push({ command, args, cwd: cwd.slice(root.length + 1) });
    if (args[0] === "run") return "";
    const out = args[args.indexOf("--pack-destination") + 1];
    const pkg = JSON.parse(readFileSync(join(cwd, "package.json"), "utf8"));
    const dir = cwd.slice(root.length + 1);
    const entries = [];
    const manifest = structuredClone(pkg);
    if (!keepWorkspace && manifest.dependencies) {
      for (const dep of Object.keys(manifest.dependencies)) manifest.dependencies[dep] = V;
    }
    entries.push({
      name: "package/package.json",
      data: Buffer.from(JSON.stringify(manifest)),
      mode: 0o644,
    });
    if (PLATFORM[dir]) {
      const binary = PLATFORM[dir].binary;
      entries.push({
        name: `package/${binary}`,
        data: readFileSync(join(cwd, binary)),
        mode: binaryMode,
      });
      try {
        entries.push({
          name: "package/zfb-css.node",
          data: readFileSync(join(cwd, "zfb-css.node")),
          mode: 0o644,
        });
      } catch {
        // no addon in this tree
      }
    }
    if (dir === "crates/zfb-md-wasm/npm" && wasm) {
      for (const [path, data] of Object.entries(wasm.files))
        entries.push({ name: `package/${path}`, data, mode: 0o644 });
      entries.push({
        name: "package/dist/shipped-artifacts.json",
        data: wasm.manifest,
        mode: 0o644,
      });
    }
    const file = `${pkg.name.replace(/^@/, "").replace("/", "-")}-${pkg.version}.tgz`;
    writeFileSync(join(out, file), makeTarGz(entries));
    return command === "npm" ? JSON.stringify([{ filename: file }]) : `${join(out, file)}\n`;
  };
  return { run, calls };
}

const pack = (root, opts = {}) => {
  const out = join(tmp("zfb-pack-out-"), "tarballs");
  const fake = fakeRun(root, opts.fake);
  const manifest = packRelease({
    root,
    out,
    mode: "mac-local",
    sourceCommit: commitFixture(root),
    localBuilt: ["packages/zfb-darwin-arm64", "packages/zfb-darwin-x64"],
    run: fake.run,
    log: () => {},
    ...opts.pack,
  });
  return { manifest, out, calls: fake.calls };
};

describe("pack-release-tarballs", () => {
  it("packs only ten active packages from the source policy, preserving arm64 build files", () => {
    const root = tree();
    mkdirSync(join(root, "release"));
    writeFileSync(
      join(root, "release/npm-publication-policy.json"),
      JSON.stringify({
        schema: "zfb-npm-publication-policy/v1",
        suspended: ["@takazudo/zfb-linux-arm64-gnu"],
      }),
    );
    const { manifest, calls } = pack(root);
    expect(manifest.packages).toHaveLength(10);
    expect(manifest.packages.map((p) => p.name)).not.toContain("@takazudo/zfb-linux-arm64-gnu");
    expect(calls.some((c) => c.cwd === "packages/zfb-linux-arm64-gnu")).toBe(false);
    expect(readFileSync(join(root, "packages/zfb-linux-arm64-gnu/zfb"))).toBeTruthy();
  });
  it("packs all 11 in publication order with integrity and provenance decisions", () => {
    const { manifest, out, calls } = pack(tree());
    expect(manifest.schema).toBe("zfb-npm-tarballs/v1");
    expect(manifest.packages.map((p) => p.dir)).toEqual([...Object.keys(PLATFORM), ...WORKSPACE]);
    for (const p of manifest.packages) {
      const bytes = readFileSync(join(out, p.file));
      expect(p.integrity).toBe(`sha512-${createHash("sha512").update(bytes).digest("base64")}`);
      expect(p.provenance).toBe(p.dir.includes("darwin") ? "none" : "attested");
    }
    // npm packs carriers, pnpm packs workspace packages, MD/WASM without scripts.
    expect(calls.filter((c) => c.command === "npm").map((c) => c.cwd)).toEqual(
      Object.keys(PLATFORM),
    );
    expect(calls.find((c) => c.cwd === "crates/zfb-md-wasm/npm").args).toContain(
      "--ignore-scripts",
    );
    // prepublishOnly (build + test) runs before pnpm pack, which would skip it.
    expect(calls.filter((c) => c.args[0] === "run").map((c) => c.cwd)).toEqual([
      "packages/zfb",
      "packages/zfb-runtime",
    ]);
    expect(JSON.parse(readFileSync(join(out, "manifest.json"), "utf8"))).toEqual(manifest);
    expect(readFileSync(join(out, "zfb-md-wasm-shipped-artifacts.json"))).toEqual(
      wasmFiles().manifest,
    );
  });

  it.each(["all-provenance", "mac-local", "recovery-no-provenance"])(
    "stamps all active packages from SOURCE and restores manifest bytes in %s",
    (mode) => {
      const root = tree();
      const sourceCommit = commitFixture(root, { sourceMetadata: true });
      writeFileSync(
        join(root, "release/npm-publication-policy.json"),
        JSON.stringify({ schema: "zfb-npm-publication-policy/v1", suspended: [] }),
      );
      const out = join(tmp("zfb-pack-out-"), "tarballs");
      const originals = new Map(
        Object.keys(NAMES).map((dir) => [dir, readFileSync(join(root, dir, "package.json"))]),
      );
      const fake = fakeRun(root);
      const manifest = packRelease({ root, out, mode, sourceCommit, run: fake.run, log: () => {} });
      for (const { dir, file, integrity } of manifest.packages) {
        const bytes = readFileSync(join(out, file));
        const [entry] = parseTarGz(bytes).filter((entry) => entry.name === "package/package.json");
        expect(JSON.parse(entry.data.toString("utf8")).gitHead).toBe(sourceCommit);
        expect(integrity).toBe(`sha512-${createHash("sha512").update(bytes).digest("base64")}`);
        expect(readFileSync(join(root, dir, "package.json"))).toEqual(originals.get(dir));
      }
    },
  );

  it("restores exact bytes after a packer fails with the stamped manifest in place", () => {
    const root = tree();
    const path = join(root, "packages/zfb-darwin-arm64/package.json");
    const original = Buffer.concat([readFileSync(path), Buffer.from("  \r\n")]);
    writeFileSync(path, original);
    const sourceCommit = commitFixture(root, { sourceMetadata: true });
    const out = join(tmp("zfb-pack-out-"), "tarballs");
    const run = () => {
      expect(JSON.parse(readFileSync(path, "utf8")).gitHead).toBe(sourceCommit);
      throw new Error("packer failed");
    };
    expect(() => packRelease({ root, out, mode: "mac-local", sourceCommit, run })).toThrow(
      "packer failed",
    );
    expect(readFileSync(path)).toEqual(original);
  });

  it("requires full SOURCE and rejects CONTROL when it differs from the checkout", () => {
    const root = tree();
    const source = commitFixture(root, { sourceMetadata: true });
    const out = join(tmp("zfb-pack-out-"), "tarballs");
    const control = execFileSync(
      "git",
      [
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.com",
        "commit-tree",
        "HEAD^{tree}",
        "-p",
        source,
        "-m",
        "control fixture",
      ],
      { cwd: root, encoding: "utf8" },
    ).trim();
    const args = ["--root", root, "--out", out, "--mode", "recovery-no-provenance"];
    const run = vi.fn();
    expect(main(args, { run })).toBe(2);
    expect(main([...args, "--source-commit", source.slice(0, 12)], { run })).toBe(2);
    expect(main([...args, "--source-commit", control], { run })).toBe(1);
    expect(run).not.toHaveBeenCalled();
  });

  it("retains the original unstamped bytes for sources predating the contract", () => {
    const { manifest, out } = pack(tree());
    for (const { file } of manifest.packages) {
      const entry = parseTarGz(readFileSync(join(out, file))).find(
        (entry) => entry.name === "package/package.json",
      );
      expect(JSON.parse(entry.data.toString("utf8"))).not.toHaveProperty("gitHead");
    }
  });

  it("blocks pack output admission when a legacy package fails structural inspection", () => {
    const out = join(tmp("zfb-pack-rejected-"), "tarballs");
    const inspect = vi.spyOn(npmTarball, "readNpmPayload").mockImplementationOnce(() => {
      throw new Error("npm tarball duplicate file destination: package/readme.txt");
    });
    try {
      expect(() => pack(tree(), { pack: { out } })).toThrow(
        "tarball content rejected: npm tarball duplicate file destination: package/readme.txt",
      );
      expect(existsSync(join(out, "manifest.json"))).toBe(false);
      expect(existsSync(join(out, "zfb-md-wasm-shipped-artifacts.json"))).toBe(false);
    } finally {
      inspect.mockRestore();
    }
  });

  it("refuses a carrier whose packed binary lost 0755", () => {
    expect(() => pack(tree(), { fake: { binaryMode: 0o644 } })).toThrow(
      /binary zfb has mode 644, expected 755/,
    );
  });

  it("refuses an unrewritten workspace: specifier", () => {
    expect(() => pack(tree(), { fake: { keepWorkspace: true } })).toThrow(
      /dependencies\.@takazudo\/zfb-slugify is still workspace:\*/,
    );
  });

  it("refuses a missing CSS addon before packing anything", () => {
    const root = tree();
    writeFileSync(join(root, "packages/zfb-win32-x64-msvc/zfb-css.node"), "");
    const fake = fakeRun(root);
    // Present-but-empty passes the presence preflight; the packed check catches it.
    expect(() =>
      packRelease({
        root,
        out: join(tmp("o-"), "t"),
        mode: "all-provenance",
        sourceCommit: commitFixture(root),
        run: fake.run,
        log: () => {},
      }),
    ).toThrow(/zfb-css\.node is missing/);
  });

  it("refuses MD/WASM bytes that disagree with its packed manifest", () => {
    const wasm = wasmFiles();
    wasm.files[MD_WASM_PATHS[0][1]] = Buffer.from("tampered");
    expect(() => pack(tree(), { fake: { wasm } })).toThrow(
      /published bytes differ from the manifest/,
    );
  });

  it("refuses a packed MD/WASM manifest that differs from the build-wasm-md one", () => {
    expect(() => pack(tree(), { pack: { mdWasmManifest: Buffer.from("{}") } })).toThrow(
      /differs from the build-wasm-md manifest/,
    );
  });

  it("checks Darwin carriers against the admission verdict", () => {
    const verdict = (binary) => ({
      admitted: true,
      targets: {
        "aarch64-apple-darwin": {
          binary: { sha256: sha256(binary("@takazudo/zfb-darwin-arm64")) },
          addon: { sha256: sha256("css @takazudo/zfb-darwin-arm64") },
        },
        "x86_64-apple-darwin": {
          binary: { sha256: sha256(binary("@takazudo/zfb-darwin-x64")) },
          addon: { sha256: sha256("css @takazudo/zfb-darwin-x64") },
        },
      },
    });
    expect(() => pack(tree(), { pack: { verdict: verdict((n) => `bin ${n}`) } })).not.toThrow();
    expect(() => pack(tree(), { pack: { verdict: verdict((n) => `other ${n}`) } })).toThrow(
      /package\/zfb sha256 .* != admitted/,
    );
  });

  it("recovery of an older tag may omit only zfb-slugify; other modes refuse", () => {
    const root = tree({ omit: ["packages/zfb-slugify"] });
    expect(inventory(root, policyForTree(root, "recovery-no-provenance"))).not.toContain(
      "packages/zfb-slugify",
    );
    expect(() => inventory(root, policyForTree(root, "mac-local"))).toThrow(
      /missing expected package manifest: packages\/zfb-slugify/,
    );
    const { manifest } = pack(root, { pack: { mode: "recovery-no-provenance", localBuilt: [] } });
    expect(manifest.packages).toHaveLength(10);
    expect(manifest.packages.every((p) => p.provenance === "none")).toBe(true);
  });

  it("refuses a non-empty output directory", () => {
    const root = tree();
    const out = tmp("zfb-pack-dirty-");
    writeFileSync(join(out, "stale.tgz"), "x");
    expect(() =>
      packRelease({
        root,
        out,
        mode: "mac-local",
        sourceCommit: commitFixture(root),
        run: fakeRun(root).run,
        log: () => {},
      }),
    ).toThrow(/must be empty/);
  });

  it("validates local-built dirs and derives the same decisions as the publish helper", () => {
    expect(parseLocalBuilt("mac-local", "")).toEqual(["packages/zfb-darwin-x64"]);
    expect(() => parseLocalBuilt("mac-local", "packages/zfb")).toThrow(
      /not a platform package dir/,
    );
    expect(() => parseLocalBuilt("recovery-no-provenance", "packages/zfb-darwin-x64")).toThrow();
    expect(provenanceFor("packages/zfb", "recovery-no-provenance", [])).toBe("none");
    expect(provenanceFor("packages/zfb-darwin-x64", "mac-local", ["packages/zfb-darwin-x64"])).toBe(
      "none",
    );
    expect(provenanceFor("packages/zfb", "mac-local", ["packages/zfb-darwin-x64"])).toBe(
      "attested",
    );
  });
});
