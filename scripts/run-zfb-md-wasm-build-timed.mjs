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
const targetDir = process.env.CARGO_TARGET_DIR;
if (!targetDir) {
  throw new Error("CARGO_TARGET_DIR must name a distinct initially nonexistent directory");
}
if (existsSync(targetDir)) {
  throw new Error(
    `CARGO_TARGET_DIR already exists; clean-run timing requires a nonexistent path: ${targetDir}`,
  );
}
const originalExecFileSync = childProcess.execFileSync;
const timings = [];

function productionStep(command, args) {
  if (command === "cargo" && args[0] === "rustc") return "cargo rustc";
  if (command === "wasm-bindgen") return "wasm-bindgen";
  if (command.endsWith("/wasm-opt") || command.endsWith("\\wasm-opt.exe")) return "wasm-opt";
  return null;
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
