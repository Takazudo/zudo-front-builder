#!/usr/bin/env node
/**
 * scripts/build.mjs — builds @takazudo/zfb-md-wasm (zfb#1577, epic zfb#1572).
 *
 * Builds TWO wasm artifacts (zfb#1849, epic zfb#1845):
 *   - the default artifact (all default-on Cargo features -- compile,
 *     renderHtml, highlightCode) under `src/wasm/`, served by the package's
 *     `.` entry.
 *   - a highlight-only artifact (`cargo ... --no-default-features`, which
 *     drops the `pipeline` feature -- see crates/zfb-md-wasm/Cargo.toml)
 *     under `src/wasm-highlight/`, served by the package's `./highlight`
 *     entry. The md/MDX/JSX pipeline dominates the shipped bytes (Wave-1
 *     measurement on epic #1845: 51.8% raw / 44.5% gzip of the default
 *     artifact) -- dropping it is the size knob for consumers that only
 *     need syntax highlighting.
 *
 * Each artifact goes through the same 3-step pipeline:
 *   1. cargo rustc --target wasm32-unknown-unknown --profile wasm-release
 *      -p zfb-md-wasm --crate-type cdylib [--no-default-features]
 *      (the size-optimized profile added to the repo root Cargo.toml by
 *      this issue — opt-level "z", LTO, 1 codegen unit, panic=abort;
 *      opt-in via --profile so it never changes the default `release`
 *      profile other crates/binaries build with. `rustc --crate-type
 *      cdylib` because the manifest is rlib-only — see Cargo.toml)
 *   2. wasm-bindgen --target web                (ESM glue, browser + Node)
 *   3. wasm-opt -O1 (binaryen, pinned via the `binaryen` devDependency)
 *
 * Sequencing between the two artifacts is load-bearing: both cargo rustc
 * invocations write the SAME cdylib path
 * (target/wasm32-unknown-unknown/wasm-release/zfb_md_wasm.wasm) — the
 * default artifact's cdylib must be fully consumed by wasm-bindgen (step 2)
 * before the highlight-only cargo rustc pass overwrites it. The two
 * artifacts are therefore built one after the other, never in parallel.
 *
 * After both artifacts:
 *   4. tsc (src/*.ts -> dist/*.js) — compiles every entry (index/browser/
 *      highlight/highlight-browser) in one pass, since it needs both
 *      src/wasm/ and src/wasm-highlight/ to already exist for the
 *      resource-URL imports to resolve.
 *   5. mark each artifact's generated glue as a zfb file-loader resource,
 *      then copy src/wasm/ -> dist/wasm/ and src/wasm-highlight/ ->
 *      dist/wasm-highlight/.
 *
 * Prints the raw + gzip size of BOTH final wasm binaries (epic #1572's
 * download-size concern; #1579's CI size-report line and #1580's docs page
 * both read these numbers back out of this script's stdout).
 *
 * Usage: node scripts/build.mjs   (run via `pnpm build` / `pnpm --filter
 * @takazudo/zfb-md-wasm build`)
 */

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, cpSync, renameSync } from "node:fs";
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
// ship over the wire. Shared by both artifacts.
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
// target resolution. Prepending the active rustup toolchain's own bin dir
// (when present) fixes this without affecting CI. Ask rustup which toolchain
// is active; directory enumeration can select an older installed toolchain.
function envWithRustupPathFix() {
  const env = { ...process.env };
  try {
    const rustcPath = execFileSync("rustup", ["which", "rustc"], {
      encoding: "utf8",
      env,
    }).trim();
    const binDir = dirname(rustcPath);
    if (existsSync(resolve(binDir, "rustc"))) {
      env.PATH = `${binDir}:${env.PATH ?? ""}`;
    }
  } catch {
    // A plain rustc installation remains usable without rustup.
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

function cargoTargetDirectory(env) {
  const metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      cwd: repoRoot,
      encoding: "utf8",
      env,
    }),
  );
  if (typeof metadata.target_directory !== "string") {
    throw new Error("cargo metadata did not report a target_directory");
  }
  return metadata.target_directory;
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

/**
 * Builds one wasm artifact end-to-end (cargo rustc -> wasm-bindgen ->
 * wasm-opt) into `srcOutDir`. `cargoFeatureArgs` is `[]` for the default
 * artifact or `["--no-default-features"]` for the highlight-only one;
 * `outName` becomes both the wasm-bindgen `--out-name` and the emitted file
 * stems (`<outName>_bg.wasm`, `<outName>_glue.zfb-resource.mjs`, …).
 */
function buildWasmArtifact({ env, label, cargoFeatureArgs, outName, srcOutDir }) {
  rmSync(srcOutDir, { recursive: true, force: true });
  mkdirSync(srcOutDir, { recursive: true });

  // `cargo rustc --crate-type cdylib`, not `cargo build`: the crate's manifest
  // declares `crate-type = ["rlib"]` (see Cargo.toml for why — a native cdylib
  // links V8 and fails as an ELF `-shared` object). The wasm cdylib is forced
  // here for the wasm32 target only, where nothing pulls V8 into the graph.
  log(
    `== ${label} 1/3: cargo rustc --target wasm32-unknown-unknown --profile wasm-release -p ${crateName} --crate-type cdylib ${cargoFeatureArgs.join(" ")} ==`,
  );
  run(
    "cargo",
    [
      "rustc",
      "--target",
      "wasm32-unknown-unknown",
      "--profile",
      "wasm-release",
      "-p",
      crateName,
      "--crate-type",
      "cdylib",
      ...cargoFeatureArgs,
    ],
    {
      env,
    },
  );
  // Both artifacts' cargo rustc pass writes this SAME path (see module doc
  // "Sequencing" note) -- read it immediately, before the next artifact's
  // cargo rustc pass (if any) overwrites it.
  const cdylibPath = resolve(
    cargoTargetDirectory(env),
    "wasm32-unknown-unknown/wasm-release/zfb_md_wasm.wasm",
  );
  const cdylibSize = readFileSync(cdylibPath).length;
  log(`${label} cdylib (pre wasm-bindgen): ${fmtBytes(cdylibSize)}`);

  log(`== ${label} 2/3: wasm-bindgen --target web ==`);
  run(
    "wasm-bindgen",
    ["--target", "web", "--out-dir", srcOutDir, "--out-name", outName, cdylibPath],
    {
      env,
    },
  );
  const bgWasmPath = resolve(srcOutDir, `${outName}_bg.wasm`);
  const gluePath = resolve(srcOutDir, `${outName}.js`);
  const glueDeclarationPath = resolve(srcOutDir, `${outName}.d.ts`);
  const resourceGluePath = resolve(srcOutDir, `${outName}_glue.zfb-resource.mjs`);
  const resourceGlueDeclarationPath = resolve(srcOutDir, `${outName}_glue.zfb-resource.d.mts`);

  // One canonical generated runtime serves both entries. The marker turns the
  // browser entry's static import into an esbuild file resource, while the
  // direct entry dynamically imports this exact same module.
  renameSync(gluePath, resourceGluePath);
  renameSync(glueDeclarationPath, resourceGlueDeclarationPath);
  const bindgenSize = readFileSync(bgWasmPath).length;
  log(`${label} wasm-bindgen output: ${fmtBytes(bindgenSize)}`);

  log(`== ${label} 3/3: wasm-opt ${WASM_OPT_LEVEL} ==`);
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
  log(`${label} wasm-opt output: ${fmtBytes(finalSize)}`);
  log(`${label} gzip -9: ${fmtBytes(gzipSize)}`);

  return { cdylibSize, bindgenSize, finalSize, gzipSize };
}

function main() {
  const env = envWithRustupPathFix();
  checkWasmBindgenVersion(env);

  const distDir = resolve(pkgRoot, "dist");
  rmSync(distDir, { recursive: true, force: true });

  const defaultStats = buildWasmArtifact({
    env,
    label: "default",
    cargoFeatureArgs: [],
    outName: "zfb_md_wasm",
    srcOutDir: resolve(pkgRoot, "src/wasm"),
  });

  const highlightStats = buildWasmArtifact({
    env,
    label: "highlight-only",
    cargoFeatureArgs: ["--no-default-features"],
    outName: "zfb_md_wasm_highlight",
    srcOutDir: resolve(pkgRoot, "src/wasm-highlight"),
  });

  log(`== tsc ==`);
  run(resolve(pkgRoot, "node_modules/.bin/tsc"), [], { cwd: pkgRoot, env });

  for (const dirName of ["wasm", "wasm-highlight"]) {
    const distSubDir = resolve(distDir, dirName);
    mkdirSync(distSubDir, { recursive: true });
    cpSync(resolve(pkgRoot, "src", dirName), distSubDir, { recursive: true });
  }

  console.log("");
  console.log("== zfb-md-wasm build summary ==");
  console.log("-- default artifact (`.` entry) --");
  console.log(`cdylib (cargo, wasm-release profile): ${fmtBytes(defaultStats.cdylibSize)}`);
  console.log(`wasm-bindgen --target web:             ${fmtBytes(defaultStats.bindgenSize)}`);
  console.log(
    `wasm-opt ${WASM_OPT_LEVEL} (final):              ${fmtBytes(defaultStats.finalSize)}`,
  );
  console.log(`gzip -9 (final):                       ${fmtBytes(defaultStats.gzipSize)}`);
  console.log("-- highlight-only artifact (`./highlight` entry) --");
  console.log(`cdylib (cargo, wasm-release profile): ${fmtBytes(highlightStats.cdylibSize)}`);
  console.log(`wasm-bindgen --target web:             ${fmtBytes(highlightStats.bindgenSize)}`);
  console.log(
    `wasm-opt ${WASM_OPT_LEVEL} (final):              ${fmtBytes(highlightStats.finalSize)}`,
  );
  console.log(`gzip -9 (final):                       ${fmtBytes(highlightStats.gzipSize)}`);
}

main();
