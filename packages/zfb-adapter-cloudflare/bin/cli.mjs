#!/usr/bin/env node
//
// `zfb-adapter-cloudflare` CLI.
//
// Subcommands:
//
//   bundle <input> --outdir <dir>
//
//     Wrap the input ESM bundle into a Cloudflare Workers Static Assets
//     (Pages-compatible) `_worker.js` placed under <dir>, alongside a
//     `.assetsignore` that excludes the wrapper and inner bundle from
//     the asset upload. The input bundle is the file `zfb_build`'s
//     bundler emits; <dir> is typically the project's `dist/`.
//
// The CLI is intentionally tiny and dependency-free. It imports the
// wrapper string from the canonical `src/worker-wrapper.mjs` (plain JS,
// no TypeScript loader required) so there is a single source of truth.
// invariant: no runtime npm deps — see SECURITY-DEPS.md

import { realpathSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// ---------------------------------------------------------------------------
// Wrapper source — imported from the single canonical source.
// ---------------------------------------------------------------------------

import { WORKER_WRAPPER_SOURCE } from "../src/worker-wrapper.mjs";
export { WORKER_WRAPPER_SOURCE };

// ---------------------------------------------------------------------------
// emitWorker — shared implementation, no runtime npm deps.
// ---------------------------------------------------------------------------

import { emitWorker as _emitWorker } from "../src/emit-worker.mjs";

export async function emitWorker({ inputBundlePath, outdir }) {
  return _emitWorker({ inputBundlePath, outdir, workerWrapperSource: WORKER_WRAPPER_SOURCE });
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
Cloudflare Workers Static Assets \`_worker.js\` placed under <dir>
(also deployable to Cloudflare Pages advanced mode).

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
  // WORKER_WRAPPER_SOURCE). When this file is run directly the process
  // entry resolves (after symlinks) to this file's realpath.
  //
  // pnpm's `.bin` shim invokes `node node_modules/.bin/../<pkg>/bin/cli.mjs`,
  // so `process.argv[1]` lexically resolves to the symlinked
  // `node_modules/<pkg>/bin/cli.mjs` path — but Node's ESM loader follows
  // symlinks by default, so `import.meta.url` points at the realpath under
  // `.pnpm/`. A lexical compare therefore mismatches under pnpm exec and
  // the CLI silently no-ops. Compare realpaths so direct `node bin/cli.mjs`,
  // pnpm exec, npm bin shims, and Yarn PnP all agree.
  const entry = process.argv[1];
  if (!entry) return;
  let entryReal;
  let selfReal;
  try {
    entryReal = realpathSync(entry);
    selfReal = realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return; // can't resolve either side — treat as imported, not run
  }
  if (entryReal !== selfReal) return;

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
  process.stdout.write(
    `wrote ${out.workerPath}\nwrote ${out.innerBundlePath}\nwrote ${out.assetsIgnorePath}\n`,
  );
}

main().catch((err) => {
  const msg = err instanceof Error ? (err.stack ?? err.message) : String(err);
  fail(msg);
});
