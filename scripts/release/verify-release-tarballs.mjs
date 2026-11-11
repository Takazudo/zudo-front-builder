#!/usr/bin/env node
// verify-release-tarballs.mjs — the publish job's independent re-inspection of
// the pack job's tarballs (#4690). The job that holds the npm OIDC credential
// does not trust pack's own checks: before any `npm publish` it re-runs the
// same content inspection on every tarball, with control scripts and Node
// builtins only, against inputs the publish job downloads itself.
//
//   node scripts/release/verify-release-tarballs.mjs --dir TARBALLS --version V
//        --admission verdict.json [--md-wasm-manifest FILE | --allow-missing-md-wasm-manifest]
//
// Per manifest row:
//   - the dir names exactly its known package (a row cannot relabel a carrier);
//   - the file's sha512 equals the row's integrity;
//   - row version and packed package.json name/version equal the release
//     version and the row (inspectTarball);
//   - no `workspace:` specifier; non-Windows carriers' `zfb` is 0755; every
//     carrier has zfb-css.node when the packed @takazudo/zfb exports ./css;
//   - Darwin carriers' zfb and zfb-css.node equal the admission verdict's
//     digests (a verdict is required);
//   - MD/WASM's four WASM files match its packed manifest, and that manifest
//     equals build-wasm-md's artifact (required unless explicitly allowed
//     missing, for recovery of a tag that predates it).
//
// Exit 0 verified, 1 a check failed, 2 usage or read error.
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { parseTarGz } from "../admit-darwin-artifacts.mjs";
import { isMain } from "./gh.mjs";
import { PLATFORM, SCHEMA, inspectTarball } from "./pack-release-tarballs.mjs";

export const KNOWN_PACKAGES = Object.freeze({
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

const integrityOf = (bytes) => `sha512-${createHash("sha512").update(bytes).digest("base64")}`;

/** Does the packed @takazudo/zfb export ./css (so every carrier needs its addon)? */
export function packedExportsCss(bytes) {
  const entry = parseTarGz(bytes).find((e) => e.name === "package/package.json");
  const pkg = JSON.parse(entry?.data.toString("utf8") ?? "{}");
  return Boolean(pkg.exports?.["./css"] ?? pkg.publishConfig?.exports?.["./css"]);
}

export function verifyTarballs({
  dir,
  version,
  verdict,
  mdWasmManifest,
  allowMissingMdWasmManifest,
}) {
  const problems = [];
  const manifest = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8"));
  if (manifest.schema !== SCHEMA) return [`manifest schema is ${manifest.schema}`];
  if (!mdWasmManifest && !allowMissingMdWasmManifest) {
    problems.push("no build-wasm-md manifest to compare the packed MD/WASM manifest against");
  }
  const bytesOf = new Map();
  for (const row of manifest.packages ?? []) {
    const path = join(dir, String(row.file ?? ""));
    if (!row.file || String(row.file).includes("/") || !existsSync(path)) {
      problems.push(`${row.name}: tarball ${row.file} is missing`);
      continue;
    }
    const bytes = readFileSync(path);
    bytesOf.set(row, bytes);
    if (KNOWN_PACKAGES[row.dir] !== row.name) {
      problems.push(`${row.name}: manifest dir ${row.dir} does not belong to this package`);
    }
    if (row.version !== version)
      problems.push(`${row.name}: manifest version ${row.version} is not ${version}`);
    if (integrityOf(bytes) !== row.integrity)
      problems.push(`${row.name}: tarball sha512 differs from the manifest`);
  }
  const zfbRow = [...bytesOf.keys()].find((row) => row.name === "@takazudo/zfb");
  let hasCss = false;
  try {
    hasCss = zfbRow ? packedExportsCss(bytesOf.get(zfbRow)) : false;
  } catch (error) {
    problems.push(`@takazudo/zfb: unreadable packed package.json: ${error.message}`);
  }
  for (const [row, bytes] of bytesOf) {
    if (PLATFORM[row.dir]?.triple && !verdict?.admitted) {
      problems.push(`${row.name}: no admitted Darwin verdict to check the carrier against`);
      continue;
    }
    const inspected = inspectTarball(bytes, {
      dir: row.dir,
      name: row.name,
      version,
      hasCss,
      verdict,
      mdWasmManifest,
    });
    for (const problem of inspected.problems) problems.push(`${row.name}: ${problem}`);
  }
  return problems;
}

export function main(argv, { out = process.stdout, err = process.stderr } = {}) {
  const opts = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--allow-missing-md-wasm-manifest") opts.allowMissing = true;
    else if (
      ["--dir", "--version", "--admission", "--md-wasm-manifest"].includes(arg) &&
      argv[i + 1]
    ) {
      opts[arg.slice(2)] = argv[++i];
    } else {
      err.write(`verify-release-tarballs: unexpected argument ${arg}\n`);
      return 2;
    }
  }
  if (!opts.dir || !opts.version) {
    err.write(
      "usage: verify-release-tarballs.mjs --dir DIR --version V --admission FILE [--md-wasm-manifest FILE | --allow-missing-md-wasm-manifest]\n",
    );
    return 2;
  }
  let problems;
  try {
    problems = verifyTarballs({
      dir: opts.dir,
      version: opts.version,
      verdict: opts.admission ? JSON.parse(readFileSync(opts.admission, "utf8")) : null,
      mdWasmManifest: opts["md-wasm-manifest"] ? readFileSync(opts["md-wasm-manifest"]) : null,
      allowMissingMdWasmManifest: Boolean(opts.allowMissing),
    });
  } catch (error) {
    err.write(`verify-release-tarballs: ${error.message}\n`);
    return 2;
  }
  if (problems.length) {
    for (const problem of problems) out.write(`::error::${problem}\n`);
    return 1;
  }
  out.write(`Re-verified every packed tarball in ${opts.dir} for ${opts.version}.\n`);
  return 0;
}

if (isMain(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
