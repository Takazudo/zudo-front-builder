#!/usr/bin/env node
/**
 * scripts/build.mjs — builds @takazudo/zfb-md-wasm (zfb#1577, epic zfb#1572).
 *
 * Builds four isolated wasm artifacts sequentially:
 *   - the default artifact (all default-on Cargo features -- compile,
 *     renderHtml, parseToAst, highlightCode) under `src/wasm/`, served by the
 *     package's `.` entry.
 *   - explicit highlight, render, and parse singleton feature artifacts under
 *     `src/wasm-highlight/`, `src/wasm-render/`, and `src/wasm-parse/`.
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
 * Sequencing between the four artifacts is load-bearing: every cargo rustc
 * invocation writes the SAME cdylib path
 * (target/wasm32-unknown-unknown/wasm-release/zfb_md_wasm.wasm) — the
 * default artifact's cdylib must be fully consumed by wasm-bindgen (step 2)
 * before the next cargo rustc pass overwrites it. The four artifacts are
 * therefore built one after the other, never in parallel.
 *
 * After all four artifacts:
 *   4. tsc (src/*.ts -> dist/*.js) — compiles every entry (index/browser/
 *      and every direct/browser singleton) in one pass, after all generated
 *      resource directories exist.
 *   5. mark each artifact's generated glue as a zfb file-loader resource,
 *      then copy all four source resource directories into `dist`.
 *
 * Prints raw, generated-glue, final, and gzip sizes for every artifact (epic
 * #1572's download-size concern; #1579's CI size-report line and #1580's docs
 * page read these numbers back out of this script's stdout).
 *
 * Usage: node scripts/build.mjs   (run via `pnpm build` / `pnpm --filter
 * @takazudo/zfb-md-wasm build`)
 */

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, cpSync, renameSync, readdirSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
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
// ship over the wire. Shared by all four artifacts.
const WASM_OPT_LEVEL = "-O1";

export const ARTIFACTS = [
  {
    label: "default",
    entry: ".",
    cargoFeatureArgs: [],
    outName: "zfb_md_wasm",
    dirName: "wasm",
    gzipCeiling: 1_600_000,
  },
  {
    label: "highlight-only",
    entry: "./highlight",
    cargoFeatureArgs: ["--no-default-features", "--features", "highlight"],
    outName: "zfb_md_wasm_highlight",
    dirName: "wasm-highlight",
    gzipCeiling: 820_000,
  },
  {
    label: "render-only",
    entry: "./render",
    cargoFeatureArgs: ["--no-default-features", "--features", "render"],
    outName: "zfb_md_wasm_render",
    dirName: "wasm-render",
    gzipCeiling: 1_100_000,
  },
  {
    label: "parse-only",
    entry: "./parse",
    cargoFeatureArgs: ["--no-default-features", "--features", "parse"],
    outName: "zfb_md_wasm_parse",
    dirName: "wasm-parse",
    gzipCeiling: 325_000,
  },
];

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
 * artifact or the exact singleton feature arguments for a slim artifact;
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
  const glueBytes = readFileSync(resourceGluePath);
  const glueSize = glueBytes.length;
  const glueGzipSize = gzipSync(glueBytes, { level: 9 }).length;
  log(`${label} wasm-bindgen output: ${fmtBytes(bindgenSize)}`);
  log(`${label} generated glue: ${fmtBytes(glueSize)}`);
  log(`${label} generated glue gzip -9: ${fmtBytes(glueGzipSize)}`);

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

  const expectedFiles = [
    `${outName}_glue.zfb-resource.mjs`,
    `${outName}_glue.zfb-resource.d.mts`,
    `${outName}_bg.wasm`,
    `${outName}_bg.wasm.d.ts`,
  ].sort();
  const actualFiles = readdirSync(srcOutDir).sort();
  if (JSON.stringify(actualFiles) !== JSON.stringify(expectedFiles)) {
    throw new Error(
      `${label} generated resource set is not closed: expected ${expectedFiles.join(", ")}; ` +
        `received ${actualFiles.join(", ")}`,
    );
  }

  return { cdylibSize, bindgenSize, glueSize, glueGzipSize, finalSize, gzipSize };
}

function main() {
  const env = envWithRustupPathFix();
  checkWasmBindgenVersion(env);

  const distDir = resolve(pkgRoot, "dist");
  rmSync(distDir, { recursive: true, force: true });

  const stats = ARTIFACTS.map((artifact) => ({
    artifact,
    stats: buildWasmArtifact({
      env,
      label: artifact.label,
      cargoFeatureArgs: artifact.cargoFeatureArgs,
      outName: artifact.outName,
      srcOutDir: resolve(pkgRoot, "src", artifact.dirName),
    }),
  }));

  log(`== tsc ==`);
  run(resolve(pkgRoot, "node_modules/.bin/tsc"), [], { cwd: pkgRoot, env });

  for (const { dirName } of ARTIFACTS) {
    const distSubDir = resolve(distDir, dirName);
    mkdirSync(distSubDir, { recursive: true });
    cpSync(resolve(pkgRoot, "src", dirName), distSubDir, { recursive: true });
  }

  console.log("");
  console.log("== zfb-md-wasm build summary ==");
  for (const { artifact, stats: artifactStats } of stats) {
    console.log(`-- ${artifact.label} artifact (\`${artifact.entry}\` entry) --`);
    console.log(`raw cdylib:                            ${fmtBytes(artifactStats.cdylibSize)}`);
    console.log(`wasm-bindgen binary:                   ${fmtBytes(artifactStats.bindgenSize)}`);
    console.log(`generated glue:                        ${fmtBytes(artifactStats.glueSize)}`);
    console.log(`generated glue gzip -9:                ${fmtBytes(artifactStats.glueGzipSize)}`);
    console.log(
      `wasm-opt ${WASM_OPT_LEVEL} (final):              ${fmtBytes(artifactStats.finalSize)}`,
    );
    console.log(`gzip -9 (final):                       ${fmtBytes(artifactStats.gzipSize)}`);
    if (artifactStats.gzipSize > artifact.gzipCeiling) {
      throw new Error(
        `${artifact.label} gzip-9 size ${artifactStats.gzipSize} exceeds ceiling ${artifact.gzipCeiling}`,
      );
    }
  }
}

const argument = process.argv[1];
if (argument !== undefined && import.meta.url === pathToFileURL(argument).href) {
  main();
}
