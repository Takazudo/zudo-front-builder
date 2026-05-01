// AUTO-LOADED by zfb::config (Wave 2 / Sub 1 — `zfb.config.ts` loader,
// extended by Phase B Sub 3 / issue #108 with plugin-name resolution).
// Do not edit unless you also update the Rust caller in
// `crates/zfb/src/config.rs::load_ts_via_subprocess`.
//
// Usage:
//
//     node config-loader.mjs <bundle-path> <project-root>
//
// Behaviour:
//
// 1. Dynamic-imports the ESM bundle that the Rust caller produced via
//    esbuild (with `--bundle --format=esm --platform=node`).
// 2. Reads the module's `default` export.
// 3. Walks `default.plugins` (when present) and resolves each
//    `plugins[i].name` to an absolute module specifier the Rust side
//    can hand to the plugin host:
//      - `./` / `../` — resolved relative to `<project-root>`.
//      - `/` — treated as an absolute filesystem path verbatim.
//      - everything else — resolved as a Node bare-specifier via
//        `import.meta.resolve(name, projectRootDirURL)`.
// 4. Writes a JSON envelope `{ "config": <default>, "plugins": [...] }`
//    to stdout (single line, no trailing newline). The Rust caller
//    pipes this into `serde_json::from_str` against an envelope type
//    and stores the resolved module paths on `Config.plugins`.
// 5. Any thrown error is logged to stderr with a `[zfb-config-loader]`
//    prefix and the process exits non-zero so the caller surfaces a
//    clean failure.

import { createRequire } from "node:module";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { isAbsolute, resolve as pathResolve } from "node:path";

const { argv, exit, stdout, stderr } = process;

const bundlePath = argv[2];
const projectRoot = argv[3];
if (!bundlePath || !projectRoot) {
  stderr.write("[zfb-config-loader] missing bundle-path or project-root argument\n");
  exit(2);
}

function fail(msg, code) {
  stderr.write(`[zfb-config-loader] ${msg}\n`);
  exit(code ?? 4);
}

// Resolve one `plugins[i].name` to an absolute filesystem path. The Rust
// side stores this and hands it back to the plugin host as the literal
// argument to `import()`. Returning a fileURL string is the safest cross-
// platform shape (Windows-compatible without escaping).
function resolvePluginName(name) {
  if (typeof name !== "string" || name.length === 0) {
    throw new Error(`plugin "name" must be a non-empty string (got ${JSON.stringify(name)})`);
  }
  // Absolute filesystem path — verbatim.
  if (isAbsolute(name)) {
    return pathToFileURL(name).href;
  }
  // Path-relative — resolve against the project root.
  if (name.startsWith("./") || name.startsWith("../")) {
    return pathToFileURL(pathResolve(projectRoot, name)).href;
  }
  // Bare specifier — resolve via Node's project-root require, which
  // honours the project's `node_modules`. `createRequire` against
  // `<projectRoot>/<dummy>` walks up the way npm packages expect.
  const req = createRequire(pathResolve(projectRoot, "noop.js"));
  let resolved;
  try {
    resolved = req.resolve(name);
  } catch (err) {
    throw new Error(
      `plugin "${name}" could not be resolved as a Node bare specifier from ${projectRoot}: ${err && err.message ? err.message : err}`,
    );
  }
  return pathToFileURL(resolved).href;
}

try {
  const mod = await import(pathToFileURL(bundlePath).href);
  if (!("default" in mod)) {
    fail("config module must `export default { ... }` (no default export found)", 3);
  }
  const cfg = mod.default;
  if (cfg === null || typeof cfg !== "object" || Array.isArray(cfg)) {
    fail(
      `default export must be a plain object (got ${cfg === null ? "null" : Array.isArray(cfg) ? "array" : typeof cfg})`,
      3,
    );
  }

  // Walk `plugins[]` and produce one resolved-module-specifier entry
  // per declared plugin. We keep input order so the Rust side can zip
  // resolution results onto `Config.plugins` by index without keying on
  // names (names need not be unique on the loader side — uniqueness is
  // a build-time concern on the Rust side if we ever want to enforce it).
  const inputPlugins = Array.isArray(cfg.plugins) ? cfg.plugins : [];
  const resolvedPlugins = inputPlugins.map((p, i) => {
    if (p === null || typeof p !== "object") {
      throw new Error(`plugins[${i}] must be an object with a "name" field (got ${typeof p})`);
    }
    return resolvePluginName(p.name);
  });

  // JSON serialization drops `undefined` and any function values. The
  // Rust `Config` schema is plain data only, so this is the right shape.
  const envelope = {
    config: cfg,
    plugins: resolvedPlugins,
  };
  stdout.write(JSON.stringify(envelope));
} catch (err) {
  const msg = err && err.stack ? err.stack : err && err.message ? err.message : String(err);
  stderr.write(`[zfb-config-loader] ${msg}\n`);
  exit(4);
}
