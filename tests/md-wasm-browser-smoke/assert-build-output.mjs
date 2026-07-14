/**
 * Fast build-only guard before Chromium starts. It proves the real zfb output
 * contains the exact resource shape the browser fixture will later request;
 * it does not execute a browser or duplicate the Playwright assertions.
 */

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const testRoot = dirname(fileURLToPath(import.meta.url));
const assetsDir = join(testRoot, "fixture-site", "dist", "assets");

if (!existsSync(assetsDir)) {
  throw new Error(`fixture assets directory is missing: ${assetsDir}`);
}

const assets = readdirSync(assetsDir);
const islands = assets.filter((name) => /^islands-[A-Za-z0-9_-]+\.js$/.test(name));
if (islands.length === 0) {
  throw new Error("real zfb build emitted no hashed islands bundle");
}

const glue = assets.filter(
  (name) =>
    name.startsWith("islands-resource-zfb_md_wasm_glue.zfb-resource-") && name.endsWith(".mjs"),
);
const wasm = assets.filter(
  (name) => name.startsWith("islands-resource-zfb_md_wasm_bg-") && name.endsWith(".wasm"),
);
if (glue.length !== 1 || wasm.length !== 1) {
  throw new Error(
    `expected one packed glue and wasm resource, got glue=${glue.join(", ") || "none"}, wasm=${wasm.join(", ") || "none"}`,
  );
}

const emittedJavaScript = assets
  .filter((name) => name.endsWith(".js"))
  .map((name) => readFileSync(join(assetsDir, name), "utf8"))
  .join("\n");
for (const resource of [...glue, ...wasm]) {
  if (!emittedJavaScript.includes(`./${resource}`)) {
    throw new Error(
      `emitted browser JavaScript does not retain a relative resource edge to ${resource}`,
    );
  }
}
if (!emittedJavaScript.includes("?zfbMdWasmGen=")) {
  throw new Error("emitted browser JavaScript is missing the query-versioned trap recovery import");
}

console.log(`md-wasm browser build resources are ready: ${glue[0]}, ${wasm[0]}`);
