#!/usr/bin/env node
//
// `zfb-adapter-cloudflare` CLI.
//
// Subcommands:
//
//   bundle <input> --outdir <dir>
//
//     Wrap the input ESM bundle into a Cloudflare Pages `_worker.js`
//     placed under <dir>. The input bundle is the file `zfb_build`'s
//     bundler emits; <dir> is typically the project's `dist/`.
//
// The CLI is intentionally tiny and dependency-free. It imports the
// wrapper string from the canonical `src/worker-wrapper.mjs` (plain JS,
// no TypeScript loader required) so there is a single source of truth.

import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

// ---------------------------------------------------------------------------
// Wrapper source — imported from the single canonical source.
// ---------------------------------------------------------------------------

import { WORKER_WRAPPER_SOURCE } from "../src/worker-wrapper.mjs";
export { WORKER_WRAPPER_SOURCE };

export async function emitWorker({ inputBundlePath, outdir }) {
  const outdirAbs = resolve(outdir);
  const inputAbs = resolve(inputBundlePath);

  await mkdir(outdirAbs, { recursive: true });
  const innerBundlePath = join(outdirAbs, "_zfb_inner.mjs");
  await copyFile(inputAbs, innerBundlePath);

  const workerPath = join(outdirAbs, "_worker.js");
  await writeFile(workerPath, WORKER_WRAPPER_SOURCE, "utf8");

  return { workerPath, innerBundlePath };
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function fail(message) {
  process.stderr.write(`zfb-adapter-cloudflare: ${message}\n`);
  process.exit(1);
}

function printUsage() {
  process.stdout.write(`Usage:
  zfb-adapter-cloudflare bundle <input> --outdir <dir>

Wrap an ESM bundle (the output of zfb-build's bundler) into a
Cloudflare Pages \`_worker.js\` placed under <dir>.

Options:
  --outdir <dir>    Output directory. Required.
  -h, --help        Show this help.
`);
}

function parseArgs(argv) {
  const args = argv.slice(2);
  if (args.length === 0 || args[0] === "-h" || args[0] === "--help") {
    return { command: "help" };
  }
  const command = args[0];
  if (command !== "bundle") {
    fail(`unknown subcommand: ${command}\nRun \`zfb-adapter-cloudflare --help\` for usage.`);
  }

  let input = null;
  let outdir = null;
  let i = 1;
  while (i < args.length) {
    const arg = args[i];
    if (arg === "--outdir") {
      const next = args[i + 1];
      if (!next) fail("--outdir requires a directory argument");
      outdir = next;
      i += 2;
      continue;
    }
    if (arg.startsWith("--outdir=")) {
      outdir = arg.slice("--outdir=".length);
      i += 1;
      continue;
    }
    if (arg.startsWith("--")) {
      fail(`unknown option: ${arg}`);
    }
    if (input === null) {
      input = arg;
      i += 1;
      continue;
    }
    fail(`unexpected positional argument: ${arg}`);
  }

  if (!input) fail("missing required positional argument: <input>");
  if (!outdir) fail("missing required option: --outdir <dir>");

  return { command: "bundle", input, outdir };
}

async function main() {
  // Skip when imported (e.g. by the vitest that snapshots
  // WORKER_WRAPPER_SOURCE). `import.meta.url` will only equal the
  // process entry when this file is run directly via `node bin/cli.mjs`
  // (or via the `bin` shim pnpm installs).
  const entry = process.argv[1];
  if (!entry) return;
  const thisUrl = new URL("./cli.mjs", import.meta.url).pathname;
  if (resolve(entry) !== resolve(thisUrl)) return;

  const parsed = parseArgs(process.argv);
  if (parsed.command === "help") {
    printUsage();
    return;
  }

  const inputAbs = resolve(process.cwd(), parsed.input);
  const outdirAbs = resolve(process.cwd(), parsed.outdir);
  const out = await emitWorker({
    inputBundlePath: inputAbs,
    outdir: outdirAbs,
  });
  process.stdout.write(`wrote ${out.workerPath}\nwrote ${out.innerBundlePath}\n`);
}

main().catch((err) => {
  const msg = err instanceof Error ? (err.stack ?? err.message) : String(err);
  fail(msg);
});
