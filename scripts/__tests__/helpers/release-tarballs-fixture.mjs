// A real npm-tarball set as release.yml's pack job produces it, built offline:
// a package tree, a fake npm/pnpm `pack` that writes genuine gzip tarballs, and
// packRelease() writing manifest.json. Shared by the pack/verify suites and the
// publish shell suite (tests/unit/publish-npm-packages.sh), which drives
// `makeTarballSet` through `node --input-type=module`.
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PLATFORM, packRelease } from "../../release/pack-release-tarballs.mjs";
import { MD_WASM_PATHS } from "../../verify-zfb-md-wasm-published-digests.mjs";
import { makeTarGz } from "./darwin-fixture.mjs";

export const V = "6.1.0";
export const sha256 = (data) => createHash("sha256").update(Buffer.from(data)).digest("hex");
export const integrityOf = (bytes) =>
  `sha512-${createHash("sha512").update(bytes).digest("base64")}`;

export const NAMES = Object.freeze({
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
});

export function wasmFiles(version = V) {
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

/** A package tree like the pack job sees after placing binaries. */
export function makeTree(root, { css = true, version = V } = {}) {
  for (const [dir, name] of Object.entries(NAMES)) {
    mkdirSync(join(root, dir), { recursive: true });
    const pkg = { name, version };
    if (dir === "packages/zfb") {
      pkg.dependencies = { "@takazudo/zfb-slugify": "workspace:*" };
      if (css) pkg.publishConfig = { exports: { "./css": "./css.js" } };
    }
    writeFileSync(join(root, dir, "package.json"), JSON.stringify(pkg));
    if (PLATFORM[dir]) {
      writeFileSync(join(root, dir, PLATFORM[dir].binary), `bin ${name}`);
      if (css) writeFileSync(join(root, dir, "zfb-css.node"), `css ${name}`);
    }
  }
  return root;
}

/** The admission verdict matching makeTree's Darwin bytes. */
export function verdictFor() {
  const target = (name) => ({
    binary: { sha256: sha256(`bin ${name}`) },
    addon: { sha256: sha256(`css ${name}`) },
  });
  return {
    admitted: true,
    targets: {
      "aarch64-apple-darwin": target("@takazudo/zfb-darwin-arm64"),
      "x86_64-apple-darwin": target("@takazudo/zfb-darwin-x64"),
    },
  };
}

/** Fake npm/pnpm whose `pack` writes a real tarball of the dir (pnpm-style publishConfig/workspace rewrite). */
export function fakePackRun(
  root,
  { binaryMode = 0o755, keepWorkspace = false, wasm = wasmFiles() } = {},
) {
  const calls = [];
  const run = (command, args, { cwd }) => {
    calls.push({ command, args, cwd: cwd.slice(root.length + 1) });
    if (args[0] === "run") return "";
    const out = args[args.indexOf("--pack-destination") + 1];
    const pkg = JSON.parse(readFileSync(join(cwd, "package.json"), "utf8"));
    const dir = cwd.slice(root.length + 1);
    const manifest = structuredClone(pkg);
    if (manifest.publishConfig?.exports) {
      manifest.exports = manifest.publishConfig.exports;
      delete manifest.publishConfig;
    }
    if (!keepWorkspace && manifest.dependencies) {
      for (const dep of Object.keys(manifest.dependencies))
        manifest.dependencies[dep] = pkg.version;
    }
    const entries = [
      { name: "package/package.json", data: Buffer.from(JSON.stringify(manifest)), mode: 0o644 },
    ];
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

/** Pack a fresh tree into a fresh dir; returns { root, out, manifest, buildManifestPath }. */
export function makeTarballSet({
  mode = "mac-local",
  base = mkdtempSync(join(tmpdir(), "zfb-tarballs-")),
} = {}) {
  const root = makeTree(join(base, "root"));
  const out = join(base, "tarballs");
  const localBuilt =
    mode === "mac-local" ? ["packages/zfb-darwin-arm64", "packages/zfb-darwin-x64"] : [];
  const manifest = packRelease({
    root,
    out,
    mode,
    localBuilt,
    run: fakePackRun(root).run,
    log: () => {},
  });
  const buildManifestPath = join(base, "build-shipped-artifacts.json");
  writeFileSync(buildManifestPath, wasmFiles().manifest);
  const verdictPath = join(base, "verdict.json");
  writeFileSync(verdictPath, JSON.stringify(verdictFor()));
  return { base, root, out, manifest, buildManifestPath, verdictPath };
}

/** Replace one package's tarball and keep manifest.json self-consistent (an attacker's view). */
export function replaceTarball(out, name, entries) {
  const manifestPath = join(out, "manifest.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  const row = manifest.packages.find((p) => p.name === name);
  const bytes = makeTarGz(entries);
  writeFileSync(join(out, row.file), bytes);
  row.integrity = integrityOf(bytes);
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}
