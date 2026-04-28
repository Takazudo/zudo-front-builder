// AUTO-LOADED by zfb::config (Wave 2 / Sub 1 — `zfb.config.ts` loader).
// Do not edit unless you also update the Rust caller in
// `crates/zfb/src/config.rs::load_ts_via_subprocess`.
//
// Usage:
//
//     node config-loader.mjs <bundle-path>
//
// Behaviour:
//
// 1. Dynamic-imports the ESM bundle that the Rust caller produced via
//    esbuild (with `--bundle --format=esm --platform=node`).
// 2. Reads the module's `default` export.
// 3. Writes `JSON.stringify(default)` to stdout — a single line, no
//    trailing newline. The Rust caller pipes this into
//    `serde_json::from_str` against the `Config` schema.
// 4. Any thrown error is logged to stderr with a `[zfb-config-loader]`
//    prefix and the process exits non-zero so the caller surfaces a
//    clean failure.

import process from "node:process";
import { pathToFileURL } from "node:url";

const { argv, exit, stdout, stderr } = process;

const bundlePath = argv[2];
if (!bundlePath) {
  stderr.write("[zfb-config-loader] missing bundle path argument\n");
  exit(2);
}

try {
  const mod = await import(pathToFileURL(bundlePath).href);
  if (!("default" in mod)) {
    stderr.write(
      "[zfb-config-loader] config module must `export default { ... }` " +
        "(no default export found)\n",
    );
    exit(3);
  }
  const cfg = mod.default;
  if (cfg === null || typeof cfg !== "object" || Array.isArray(cfg)) {
    stderr.write(
      "[zfb-config-loader] default export must be a plain object " +
        `(got ${cfg === null ? "null" : Array.isArray(cfg) ? "array" : typeof cfg})\n`,
    );
    exit(3);
  }
  // JSON serialization drops `undefined` and any function values. The
  // Rust `Config` schema is plain data only, so this is the right shape.
  stdout.write(JSON.stringify(cfg));
} catch (err) {
  const msg = err && err.stack ? err.stack : err && err.message ? err.message : String(err);
  stderr.write(`[zfb-config-loader] ${msg}\n`);
  exit(4);
}
