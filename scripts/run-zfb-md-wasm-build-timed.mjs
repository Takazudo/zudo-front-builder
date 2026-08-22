#!/usr/bin/env node

// Run the existing four-pass production build while timing only the twelve
// production subprocesses (cargo rustc, wasm-bindgen, wasm-opt). The Cargo
// target directory is supplied by the caller and must be initially absent so
// the reported number is comparable to the locked clean-run reference.

import childProcess from "node:child_process";
import { existsSync } from "node:fs";
import { hrtime } from "node:process";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { syncBuiltinESMExports } from "node:module";

const repoRoot = resolve(fileURLToPath(new URL(".", import.meta.url)), "..");
const buildPath = resolve(repoRoot, "crates/zfb-md-wasm/npm/scripts/build.mjs");
const originalExecFileSync = childProcess.execFileSync;
const timings = [];

function productionStep(command, args) {
  if (command === "cargo" && args[0] === "rustc") return "cargo rustc";
  // build.mjs performs a preflight `wasm-bindgen --version`; only its
  // production `--target web --out-dir ... --out-name ...` invocations count.
  if (
    command === "wasm-bindgen" &&
    args.includes("--target") &&
    args.includes("web") &&
    args.includes("--out-dir") &&
    args.includes("--out-name")
  ) {
    return "wasm-bindgen";
  }
  if (command.endsWith("/wasm-opt") || command.endsWith("\\wasm-opt.exe")) return "wasm-opt";
  return null;
}

function selfTest() {
  if (productionStep("wasm-bindgen", ["--version"]) !== null) {
    throw new Error("wasm-bindgen --version preflight must not be timed");
  }
  if (
    productionStep("wasm-bindgen", [
      "--target",
      "web",
      "--out-dir",
      "/tmp/out",
      "--out-name",
      "zfb_md_wasm",
      "/tmp/input.wasm",
    ]) !== "wasm-bindgen"
  ) {
    throw new Error("production wasm-bindgen invocation was not classified");
  }
  if (productionStep("cargo", ["metadata"]) !== null) {
    throw new Error("cargo metadata must not be timed");
  }
  if (productionStep("cargo", ["rustc"]) !== "cargo rustc") {
    throw new Error("cargo rustc production invocation was not classified");
  }
  if (productionStep("/tmp/node_modules/.bin/wasm-opt", ["-O1"]) !== "wasm-opt") {
    throw new Error("wasm-opt production invocation was not classified");
  }
  const triplet = [
    ["cargo", ["rustc"]],
    ["wasm-bindgen", ["--target", "web", "--out-dir", "/tmp/out", "--out-name", "artifact"]],
    ["/tmp/node_modules/.bin/wasm-opt", ["-O1"]],
  ];
  const productionCount = Array.from({ length: 4 }, () => triplet)
    .flatMap((steps) => steps)
    .filter(([command, args]) => productionStep(command, args) !== null).length;
  if (productionCount !== 12)
    throw new Error(`expected 12 production steps, got ${productionCount}`);
  console.log("OK: production timing classifier excludes preflight and recognizes the 4x3 shape");
}

if (process.argv.includes("--self-test")) {
  selfTest();
  process.exit(0);
}

const targetDir = process.env.CARGO_TARGET_DIR;
if (!targetDir) {
  throw new Error("CARGO_TARGET_DIR must name a distinct initially nonexistent directory");
}
if (existsSync(targetDir)) {
  throw new Error(
    `CARGO_TARGET_DIR already exists; clean-run timing requires a nonexistent path: ${targetDir}`,
  );
}

childProcess.execFileSync = function timedExecFileSync(command, args = [], options) {
  const step = productionStep(command, args);
  if (!step) return originalExecFileSync.call(this, command, args, options);
  const started = hrtime.bigint();
  try {
    return originalExecFileSync.call(this, command, args, options);
  } finally {
    const elapsedSeconds = Number(hrtime.bigint() - started) / 1e9;
    timings.push({ step, elapsedSeconds });
    process.stderr.write(`[production-timing] ${step}: ${elapsedSeconds.toFixed(3)} seconds\n`);
  }
};

// build.mjs imports execFileSync as a named builtin ESM binding. Synchronize
// the patched CommonJS builtin before loading it so every production child
// process is measured without modifying the package build implementation.
syncBuiltinESMExports();
process.argv[1] = buildPath;
await import(pathToFileURL(buildPath).href);

if (timings.length !== 12) {
  throw new Error(
    `expected exactly 12 production subprocess timings (4 x cargo/wasm-bindgen/wasm-opt), got ${timings.length}`,
  );
}

const total = timings.reduce((sum, timing) => sum + timing.elapsedSeconds, 0);
console.log(`[production-timing] four-artifact production total: ${total.toFixed(3)} seconds`);
console.log(
  "[production-timing] Apple-M4 clean-v2 reference ceiling: 210 seconds " +
    "(informational on hosted runners; set ZFB_ENFORCE_PRODUCTION_TIME=1 to enforce)",
);
if (process.env.ZFB_ENFORCE_PRODUCTION_TIME === "1" && total > 210) {
  throw new Error(
    `four-artifact production time ${total.toFixed(3)}s exceeds 210s reference ceiling`,
  );
}
