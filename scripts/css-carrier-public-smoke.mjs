// Run only from the disposable packed-tarball extraction, using native Node22.0.0.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { compileCss } from "@takazudo/zfb/css";

const room = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
assert.equal(process.version, "v22.0.0");
assert.equal(`${process.platform}/${process.arch}`, process.env.CSS_EXPECTED_RUNTIME);
const cssModule = fileURLToPath(import.meta.resolve("@takazudo/zfb/css"));
assert.equal(realpathSync(cssModule), join(room, "node_modules/@takazudo/zfb/dist/css.js"));
const carrier = `@takazudo/zfb-${process.env.CSS_CARRIER}`;
assert.equal(require(`${carrier}/package.json`).version, process.env.CSS_PACKAGE_VERSION);
const addon = realpathSync(require.resolve(`${carrier}/zfb-css.node`));
assert.ok(relative(room, addon).startsWith("node_modules"));
assert.equal(
  createHash("sha256").update(readFileSync(addon)).digest("hex"),
  process.env.CSS_ADDON_SHA,
);
writeFileSync(join(room, "input.css"), ".carrier-authored { color: red; }");
const result = await compileCss({ cwd: room, input: "input.css", config: { wind: false } });
assert.match(result.css, /color:\s*red/);
assert.deepEqual(result.diagnostics, []);
console.log(
  JSON.stringify({
    status: "passed",
    verification: "packed-tarball extraction + public API resolution",
    node: process.version,
    platform: process.platform,
    arch: process.arch,
    cssModule,
    addon,
    carrier,
    addonSHA256: process.env.CSS_ADDON_SHA,
  }),
);
