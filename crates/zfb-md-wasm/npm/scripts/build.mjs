#!/usr/bin/env node
/**
 * scripts/build.mjs — builds @takazudo/zfb-md-wasm (zfb#1577, epic zfb#1572).
 *
 * Pipeline:
 *   1. cargo build --target wasm32-unknown-unknown --profile wasm-release
 *      -p zfb-md-wasm   (the size-optimized profile added to the repo root
 *      Cargo.toml by this issue — opt-level "z", LTO, 1 codegen unit,
 *      panic=abort; opt-in via --profile so it never changes the default
 *      `release` profile other crates/binaries build with)
 *   2. wasm-bindgen --target web                (ESM glue, browser + Node)
 *   3. wasm-opt -O1 (binaryen, pinned via the `binaryen` devDependency)
 *   4. tsc                                       (src/*.ts -> dist/*.js)
 *   5. copy the wasm-bindgen output into dist/wasm/ alongside it
 *
 * Prints the raw + gzip size of the final wasm binary (epic #1572's
 * download-size concern; #1579's CI size-report line and #1580's docs page
 * both read this number back out of this script's stdout).
 *
 * Usage: node scripts/build.mjs   (run via `pnpm build` / `pnpm --filter
 * @takazudo/zfb-md-wasm build`)
 */

import { execFileSync } from "node:child_process";
import { readdirSync, existsSync, mkdirSync, rmSync, cpSync, renameSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const pkgRoot = resolve(__dirname, ".."); // crates/zfb-md-wasm/npm
const repoRoot = resolve(pkgRoot, "../../.."); // repo root
const crateName = "zfb-md-wasm";

// wasm-bindgen-cli is a cargo-installed tool (not an npm package like
// `binaryen` below, so it can't be pinned via package.json/pnpm-lock.yaml).
// wasm-bindgen requires an EXACT version match between this CLI and the
// `wasm-bindgen` crate resolved in the workspace Cargo.lock -- pinned here,
// following crates/zfb-toolchain-pins' "pin + verify at use-time" pattern
// used for esbuild/wrangler/tailwindcss. Bump procedure: update this
// constant to match `grep -A1 '^name = "wasm-bindgen"$' Cargo.lock`, then
// `cargo install wasm-bindgen-cli --version <new> --locked --force`.
const EXPECTED_WASM_BINDGEN_VERSION = "0.2.121";

// wasm-opt's optimization level. -O1 was chosen empirically over -Oz/-O2/-O3:
// on this crate's output, more aggressive levels shrink the *raw* .wasm
// further but produce a *larger* gzip payload (aggressive inlining reduces
// the redundancy gzip exploits) — see this package's README "Artifact size"
// section for the measured numbers. -O1 minimized the gzip size we actually
// ship over the wire.
const WASM_OPT_LEVEL = "-O1";

function log(msg) {
  console.log(`[build] ${msg}`);
}

function run(cmd, args, opts = {}) {
  log(`+ ${cmd} ${args.join(" ")}`);
  execFileSync(cmd, args, { stdio: "inherit", cwd: repoRoot, ...opts });
}

// Local-machine quirk (not a CI concern, see crates/zfb-md-wasm/SPIKE-FINDINGS.md
// "Local-machine quirk"): on some Macs, Homebrew's rustc shadows the
// rustup-managed toolchain on PATH even under `rustup run`, breaking wasm
// target resolution. Prepending the rustup toolchain's own bin dir (when
// present) fixes this without affecting CI, where no such shadowing exists.
function envWithRustupPathFix() {
  const env = { ...process.env };
  const toolchainsDir = resolve(process.env.HOME ?? "", ".rustup/toolchains");
  if (existsSync(toolchainsDir)) {
    const entries = readdirSync(toolchainsDir).filter((name) =>
      existsSync(resolve(toolchainsDir, name, "bin")),
    );
    if (entries.length > 0) {
      const binDir = resolve(toolchainsDir, entries[0], "bin");
      env.PATH = `${binDir}:${env.PATH ?? ""}`;
    }
  }
  return env;
}

function checkWasmBindgenVersion(env) {
  let out;
  try {
    out = execFileSync("wasm-bindgen", ["--version"], { encoding: "utf8", env });
  } catch {
    throw new Error(
      `wasm-bindgen CLI not found on PATH. Install with:\n` +
        `  cargo install wasm-bindgen-cli --version ${EXPECTED_WASM_BINDGEN_VERSION} --locked`,
    );
  }
  const found = out.trim().split(/\s+/).pop();
  if (found !== EXPECTED_WASM_BINDGEN_VERSION) {
    throw new Error(
      `wasm-bindgen CLI version mismatch: found ${found}, expected ${EXPECTED_WASM_BINDGEN_VERSION} ` +
        `(must exactly match the wasm-bindgen crate version in the repo root Cargo.lock, or wasm-bindgen ` +
        `will refuse to process the .wasm file). Reinstall:\n` +
        `  cargo install wasm-bindgen-cli --version ${EXPECTED_WASM_BINDGEN_VERSION} --locked --force`,
    );
  }
  log(`wasm-bindgen ${found} OK`);
}

function resolveWasmOptBin() {
  const bin = resolve(pkgRoot, "node_modules/.bin/wasm-opt");
  if (!existsSync(bin)) {
    throw new Error(
      `wasm-opt not found at ${bin} — run \`pnpm install\` first (binaryen devDependency).`,
    );
  }
  return bin;
}

function fmtBytes(n) {
  return `${n.toLocaleString("en-US")} bytes (${(n / 1024 / 1024).toFixed(2)} MB)`;
}

function main() {
  const env = envWithRustupPathFix();
  checkWasmBindgenVersion(env);

  const srcWasmDir = resolve(pkgRoot, "src/wasm");
  const distDir = resolve(pkgRoot, "dist");
  rmSync(srcWasmDir, { recursive: true, force: true });
  rmSync(distDir, { recursive: true, force: true });
  mkdirSync(srcWasmDir, { recursive: true });

  log(
    `== 1/4: cargo build --target wasm32-unknown-unknown --profile wasm-release -p ${crateName} ==`,
  );
  run(
    "cargo",
    ["build", "--target", "wasm32-unknown-unknown", "--profile", "wasm-release", "-p", crateName],
    {
      env,
    },
  );
  const cdylibPath = resolve(
    repoRoot,
    "target/wasm32-unknown-unknown/wasm-release/zfb_md_wasm.wasm",
  );
  const cdylibSize = readFileSync(cdylibPath).length;
  log(`cdylib (pre wasm-bindgen): ${fmtBytes(cdylibSize)}`);

  log(`== 2/4: wasm-bindgen --target web ==`);
  run(
    "wasm-bindgen",
    ["--target", "web", "--out-dir", srcWasmDir, "--out-name", "zfb_md_wasm", cdylibPath],
    {
      env,
    },
  );
  const bgWasmPath = resolve(srcWasmDir, "zfb_md_wasm_bg.wasm");
  const bindgenSize = readFileSync(bgWasmPath).length;
  log(`wasm-bindgen output: ${fmtBytes(bindgenSize)}`);

  log(`== 3/4: wasm-opt ${WASM_OPT_LEVEL} ==`);
  const wasmOptBin = resolveWasmOptBin();
  const optTmpPath = `${bgWasmPath}.opt`;
  run(wasmOptBin, [
    WASM_OPT_LEVEL,
    // The rustc wasm32-unknown-unknown target emits bulk-memory / sign-ext /
    // mutable-globals / nontrapping-float-to-int instructions by default;
    // wasm-opt's validator needs these named explicitly (deliberately NOT
    // --all-features, which also accepts proposals this build never emits).
    "--enable-bulk-memory",
    "--enable-sign-ext",
    "--enable-mutable-globals",
    "--enable-nontrapping-float-to-int",
    "--strip-debug",
    "--strip-producers",
    "-o",
    optTmpPath,
    bgWasmPath,
  ]);
  renameSync(optTmpPath, bgWasmPath);
  const finalSize = readFileSync(bgWasmPath).length;
  const gzipSize = gzipSync(readFileSync(bgWasmPath), { level: 9 }).length;
  log(`wasm-opt output: ${fmtBytes(finalSize)}`);
  log(`gzip -9: ${fmtBytes(gzipSize)}`);

  log(`== 4/4: tsc ==`);
  run(resolve(pkgRoot, "node_modules/.bin/tsc"), [], { cwd: pkgRoot, env });

  const distWasmDir = resolve(distDir, "wasm");
  mkdirSync(distWasmDir, { recursive: true });
  cpSync(srcWasmDir, distWasmDir, { recursive: true });

  console.log("");
  console.log("== zfb-md-wasm build summary ==");
  console.log(`cdylib (cargo, wasm-release profile): ${fmtBytes(cdylibSize)}`);
  console.log(`wasm-bindgen --target web:             ${fmtBytes(bindgenSize)}`);
  console.log(`wasm-opt ${WASM_OPT_LEVEL} (final):              ${fmtBytes(finalSize)}`);
  console.log(`gzip -9 (final):                       ${fmtBytes(gzipSize)}`);
}

main();
