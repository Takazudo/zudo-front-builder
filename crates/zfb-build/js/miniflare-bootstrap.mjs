// AUTO-LOADED by zfb_build::renderer (T6). Do not edit unless you also
// update the renderer's stdout-handshake parser.
//
// Usage:
//
//   node miniflare-bootstrap.mjs --bundle <path-to-worker-bundle.mjs>
//
// Behaviour:
//
// 1. Imports `miniflare` from the workspace's `node_modules` (the
//    process is spawned with the workspace root as cwd by the renderer
//    so resolution Just Works).
// 2. Loads the worker bundle via `scriptPath` (`modules: true`,
//    `compatibilityDate` pinned for reproducibility).
// 3. Binds an ephemeral port (`port: 0`) on 127.0.0.1.
// 4. Once the server is ready, prints ONE JSON line on stdout:
//
//        {"event":"ready","url":"http://127.0.0.1:NNNNN/"}
//
//    The renderer reads that line, parses the URL, and starts driving
//    GET requests at the worker.
// 5. Forwards SIGTERM / SIGINT to a clean miniflare dispose, then
//    exits 0.
//
// Diagnostic stderr lines are passed through; the renderer captures
// them for the `RendererOutput::miniflare_logs` field.

import process from "node:process";
import { readFile } from "node:fs/promises";
import { Miniflare, Log, LogLevel } from "miniflare";

const { argv, exit, stdout, stderr } = process;

function parseArgs(argv) {
  const out = { bundle: null };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--bundle") {
      out.bundle = argv[++i] ?? null;
    }
  }
  if (!out.bundle) {
    stderr.write("[miniflare-bootstrap] missing --bundle <path>\n");
    exit(2);
  }
  return out;
}

const { bundle } = parseArgs(argv.slice(2));

const log = new Log(LogLevel.WARN, { prefix: "miniflare" });

// We deliberately pass `script` (inline source) instead of `scriptPath`.
// workerd's `scriptPath` resolution can fall over with "can't use ".."
// to break out of starting directory" when the bundle lives outside
// node's cwd (e.g. inside a tempdir). Passing the source as a string
// sidesteps the filesystem sandbox: workerd only sees the source bytes
// and the synthetic module name we hand it. Relative `import` inside
// the bundle is still resolved by miniflare's module loader against
// `modulesRoot` (set to the bundle's parent dir).
const bundleSource = await readFile(bundle, "utf8");
const path = await import("node:path");
const bundleAbs = path.resolve(bundle);
// Workerd sandboxes its filesystem at the subprocess cwd. If we hand
// it a `modulesRoot` *outside* the cwd (e.g. a tempdir under /tmp),
// it tries to traverse `..` past the sandbox boundary and dies with
// "can't use \"..\" to break out of starting directory". Stash the
// module source under a synthetic name rooted at the cwd so workerd
// stays inside its sandbox.
const modulesRoot = process.cwd();
const moduleName = "__zfb_bundle__.mjs";

const mf = new Miniflare({
  modules: [{ type: "ESModule", path: moduleName, contents: bundleSource }],
  modulesRoot,
  // Ephemeral loopback bind. The renderer parses the resolved URL from
  // the `ready` handshake below — no environment hardcodes a port.
  host: "127.0.0.1",
  port: 0,
  // Pinned to a stable date so workerd's runtime semantics don't drift
  // across CI workers. The renderer is read-only against this; if a
  // user project needs a newer date the wave-3 wiring (T7) can plumb
  // it through.
  compatibilityDate: "2025-01-01",
  // Enable Node.js compatibility so `node:*` imports that survive the
  // esbuild bundle are resolved by workerd's Node.js compat layer.
  // This is needed for `zfb/content`'s filesystem fallback path which
  // imports `node:fs/promises` at the top level (the snapshot bridge
  // path never calls it at runtime, but the import statement itself
  // must resolve at module load time). The flag is safe for SSG builds:
  // the Worker is ephemeral and only serves GET requests from the build
  // pipeline — no untrusted code is run.
  compatibilityFlags: ["nodejs_compat"],
  log,
});

let disposing = false;
async function shutdown(signal) {
  if (disposing) return;
  disposing = true;
  try {
    await mf.dispose();
  } catch (err) {
    stderr.write(`[miniflare-bootstrap] dispose error on ${signal}: ${err}\n`);
  }
  exit(0);
}
process.on("SIGTERM", () => shutdown("SIGTERM"));
process.on("SIGINT", () => shutdown("SIGINT"));

try {
  const url = await mf.ready;
  // Single JSON-shaped handshake line. Keep it on its own line so the
  // renderer can do line-buffered reads.
  stdout.write(JSON.stringify({ event: "ready", url: url.href }) + "\n");
} catch (err) {
  stderr.write(`[miniflare-bootstrap] startup error: ${err && err.stack ? err.stack : err}\n`);
  exit(1);
}
