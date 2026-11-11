#!/usr/bin/env node
// pack-release-tarballs.mjs — release.yml `pack` job body (#4690).
//
// Builds the exact npm tarballs that the `publish` job will publish, in a job
// WITHOUT `id-token: write`, so no dependency-install or package build/test code
// ever runs next to the npm OIDC credential. The publish job only verifies and
// uploads these bytes (scripts/publish-npm-packages.sh).
//
//   node scripts/release/pack-release-tarballs.mjs --mode <mac-local|recovery-no-provenance|all-provenance>
//        --out DIR [--root DIR] [--local-built "packages/zfb-darwin-arm64 packages/zfb-darwin-x64"]
//        [--admission verdict.json] [--md-wasm-manifest FILE] [--skip-prepublish]
//
// Per package, in publication order:
//   - workspace packages (not MD/WASM) run their `prepublishOnly` (build + test)
//     first: `pnpm publish` used to run it, `pnpm pack` does not;
//   - platform carriers: `npm pack` (keeps the binary's 0755 bit, #447/#441);
//     workspace packages: `pnpm pack` (rewrites `workspace:*`); MD/WASM is
//     packed from the prebuilt dist without lifecycle scripts;
//   - the packed bytes are checked: name/version, no `workspace:` specifier,
//     a 0755 `zfb` (non-Windows) and `zfb-css.node` in carriers, the admitted
//     Darwin digests, and MD/WASM's four WASM files against its packed
//     shipped-artifacts.json (and against --md-wasm-manifest when given).
// Writes DIR/<tarballs>, DIR/manifest.json
//   {schema, mode, version, packages: [{name, version, dir, file, integrity, provenance}]}
// and DIR/zfb-md-wasm-shipped-artifacts.json when the tree has one.
//
// Exit 0 packed, 1 a check failed, 2 usage error.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { parseTarGz } from "../admit-darwin-artifacts.mjs";
import {
  assertMdWasmManifest,
  verifyMdWasmArtifacts,
} from "../verify-zfb-md-wasm-published-digests.mjs";
import { isMain } from "./gh.mjs";
import { darwinDigestProblems } from "./npm-package-state.mjs";

export const PLATFORM = Object.freeze({
  "packages/zfb-darwin-arm64": { binary: "zfb", triple: "aarch64-apple-darwin" },
  "packages/zfb-darwin-x64": { binary: "zfb", triple: "x86_64-apple-darwin" },
  "packages/zfb-linux-arm64-gnu": { binary: "zfb" },
  "packages/zfb-linux-x64-gnu": { binary: "zfb" },
  "packages/zfb-win32-x64-msvc": { binary: "zfb.exe" },
});
export const WORKSPACE = Object.freeze([
  "packages/zfb-slugify",
  "packages/zfb",
  "packages/zfb-runtime",
  "packages/zfb-adapter-cloudflare",
  "packages/create-zfb",
  "crates/zfb-md-wasm/npm",
]);
const MD_WASM_DIR = "crates/zfb-md-wasm/npm";
export const MODES = Object.freeze(["mac-local", "recovery-no-provenance", "all-provenance"]);
export const SCHEMA = "zfb-npm-tarballs/v1";

const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));
const integrityOf = (bytes) => `sha512-${createHash("sha512").update(bytes).digest("base64")}`;

/** The same decision publish-npm-packages.sh derives (it re-checks it). */
export function provenanceFor(dir, mode, localBuilt) {
  if (mode === "recovery-no-provenance") return "none";
  if (mode === "mac-local" && localBuilt.includes(dir)) return "none";
  return "attested";
}

export function parseLocalBuilt(mode, raw) {
  const entries = String(raw ?? "")
    .split(/[\s,]+/)
    .map((entry) => entry.replace(/\/$/, ""))
    .filter(Boolean);
  if (mode !== "mac-local") {
    if (entries.length) throw new Error(`--local-built is set but mode is ${mode}`);
    return [];
  }
  const list = entries.length ? entries : ["packages/zfb-darwin-x64"];
  for (const entry of list) {
    if (!PLATFORM[entry])
      throw new Error(`--local-built entry ${entry} is not a platform package dir`);
  }
  return list;
}

/** Inventory in publication order; recovery of an older tag may lack slugify only. */
export function inventory(root, mode) {
  const dirs = [];
  for (const dir of [...Object.keys(PLATFORM), ...WORKSPACE]) {
    if (existsSync(join(root, dir, "package.json"))) dirs.push(dir);
    else if (dir === "packages/zfb-slugify" && mode === "recovery-no-provenance") continue;
    else
      throw new Error(`publish target is missing expected package manifest: ${dir}/package.json`);
  }
  return dirs;
}

/** Pure: problems in one packed tarball. */
export function inspectTarball(bytes, { dir, name, version, hasCss, verdict, mdWasmManifest }) {
  const problems = [];
  let entries;
  try {
    entries = parseTarGz(bytes);
  } catch (error) {
    return { problems: [`unreadable tarball: ${error.message}`] };
  }
  const file = (path) => entries.find((entry) => entry.name === `package/${path}`);
  let manifest = null;
  try {
    manifest = JSON.parse(file("package.json")?.data.toString("utf8") ?? "null");
  } catch {
    problems.push("packed package.json is not JSON");
  }
  if (manifest?.name !== name || manifest?.version !== version) {
    problems.push(
      `packed package.json is ${manifest?.name}@${manifest?.version}, not ${name}@${version}`,
    );
  }
  for (const field of ["dependencies", "peerDependencies", "optionalDependencies"]) {
    for (const [dep, spec] of Object.entries(manifest?.[field] ?? {})) {
      if (String(spec).startsWith("workspace:")) problems.push(`${field}.${dep} is still ${spec}`);
    }
  }
  const platform = PLATFORM[dir];
  if (platform) {
    const binary = file(platform.binary);
    if (!binary || binary.data.length === 0)
      problems.push(`carrier binary ${platform.binary} is missing or empty`);
    else if (platform.binary === "zfb" && (binary.mode & 0o777) !== 0o755) {
      problems.push(
        `binary zfb has mode ${(binary.mode & 0o777).toString(8)}, expected 755 (was pnpm used instead of npm pack?)`,
      );
    }
    if (hasCss && !file("zfb-css.node")?.data.length) problems.push("zfb-css.node is missing");
    if (platform.triple && verdict)
      problems.push(...darwinDigestProblems(entries, verdict, platform.triple));
  }
  let shipped = null;
  if (dir === MD_WASM_DIR) {
    const packedManifest = file("dist/shipped-artifacts.json");
    if (!packedManifest) {
      if (mdWasmManifest)
        problems.push("dist/shipped-artifacts.json is missing from the packed MD/WASM tarball");
    } else {
      shipped = packedManifest.data;
      try {
        const expected = JSON.parse(shipped.toString("utf8"));
        assertMdWasmManifest(expected, version);
        verifyMdWasmArtifacts(expected, (path) => {
          const entry = file(path);
          if (!entry) throw new Error(`${path} is missing from the packed MD/WASM tarball`);
          return entry.data;
        });
      } catch (error) {
        problems.push(error.message);
      }
      if (mdWasmManifest && !Buffer.from(shipped).equals(mdWasmManifest)) {
        problems.push("packed shipped-artifacts.json differs from the build-wasm-md manifest");
      }
    }
  }
  return { problems, shipped };
}

function defaultRun(command, args, { cwd }) {
  return execFileSync(command, args, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
}

export function packRelease({
  root,
  out,
  mode,
  localBuilt = [],
  verdict = null,
  mdWasmManifest = null,
  runPrepublish = true,
  run = defaultRun,
  log = (line) => process.stdout.write(`${line}\n`),
}) {
  if (!MODES.includes(mode)) throw new Error(`unknown mode ${mode}`);
  mkdirSync(out, { recursive: true });
  if (readdirSync(out).length) throw new Error(`--out ${out} must be empty`);
  const dirs = inventory(root, mode);
  const zfb = readJson(join(root, "packages/zfb/package.json"));
  const version = zfb.version;
  const hasCss = Boolean(zfb.publishConfig?.exports?.["./css"]);
  for (const dir of dirs) {
    if (PLATFORM[dir] && hasCss && !existsSync(join(root, dir, "zfb-css.node"))) {
      throw new Error(`Missing native CSS artifact: ${dir}/zfb-css.node`);
    }
    if (PLATFORM[dir] && !existsSync(join(root, dir, PLATFORM[dir].binary))) {
      throw new Error(`Missing carrier binary: ${dir}/${PLATFORM[dir].binary}`);
    }
  }
  const packages = [];
  const problems = [];
  let shippedCopy = null;
  for (const dir of dirs) {
    const pkg = readJson(join(root, dir, "package.json"));
    if (pkg.version !== version)
      throw new Error(`${pkg.name} is ${pkg.version}, not the lockstep ${version}`);
    const cwd = join(root, dir);
    let filename;
    if (PLATFORM[dir]) {
      const json = JSON.parse(
        run("npm", ["pack", "--json", "--ignore-scripts", "--pack-destination", out], { cwd }),
      );
      filename = basename(json[0].filename);
    } else {
      if (runPrepublish && dir !== MD_WASM_DIR && pkg.scripts?.prepublishOnly) {
        log(`→ ${pkg.name}: prepublishOnly (build + test) before pack`);
        run("pnpm", ["run", "prepublishOnly"], { cwd });
      }
      const args = ["pack", "--pack-destination", out];
      if (dir === MD_WASM_DIR) args.push("--ignore-scripts");
      filename = basename(run("pnpm", args, { cwd }).trim().split(/\r?\n/).at(-1));
    }
    const bytes = readFileSync(join(out, filename));
    const inspected = inspectTarball(bytes, {
      dir,
      name: pkg.name,
      version,
      hasCss,
      verdict,
      mdWasmManifest,
    });
    for (const problem of inspected.problems) problems.push(`${pkg.name}: ${problem}`);
    if (inspected.shipped) shippedCopy = inspected.shipped;
    const provenance = provenanceFor(dir, mode, localBuilt);
    packages.push({
      name: pkg.name,
      version,
      dir,
      file: filename,
      integrity: integrityOf(bytes),
      provenance,
    });
    log(`packed ${pkg.name}@${version} → ${filename} (${provenance})`);
  }
  if (problems.length) {
    const error = new Error(problems.join("\n"));
    error.problems = problems;
    throw error;
  }
  if (shippedCopy) writeFileSync(join(out, "zfb-md-wasm-shipped-artifacts.json"), shippedCopy);
  const manifest = { schema: SCHEMA, mode, version, packages };
  writeFileSync(join(out, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

export function main(argv, { run } = {}) {
  const opts = { root: ".", runPrepublish: true };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--skip-prepublish") opts.runPrepublish = false;
    else if (
      ["--mode", "--out", "--root", "--local-built", "--admission", "--md-wasm-manifest"].includes(
        arg,
      ) &&
      argv[i + 1] !== undefined
    ) {
      opts[arg.slice(2)] = argv[++i];
    } else {
      process.stderr.write(`pack-release-tarballs: unexpected argument ${arg}\n`);
      return 2;
    }
  }
  if (!opts.mode || !opts.out) {
    process.stderr.write(
      "usage: pack-release-tarballs.mjs --mode MODE --out DIR [--root DIR] [--local-built LIST] [--admission FILE] [--md-wasm-manifest FILE] [--skip-prepublish]\n",
    );
    return 2;
  }
  try {
    packRelease({
      root: resolve(opts.root),
      out: resolve(opts.out),
      mode: opts.mode,
      localBuilt: parseLocalBuilt(opts.mode, opts["local-built"]),
      verdict: opts.admission ? readJson(opts.admission) : null,
      mdWasmManifest: opts["md-wasm-manifest"] ? readFileSync(opts["md-wasm-manifest"]) : null,
      runPrepublish: opts.runPrepublish,
      ...(run ? { run } : {}),
    });
  } catch (error) {
    for (const line of String(error.message).split("\n"))
      process.stdout.write(`::error::${line}\n`);
    return 1;
  }
  return 0;
}

if (isMain(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
