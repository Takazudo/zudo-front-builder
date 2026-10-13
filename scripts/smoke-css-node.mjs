import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
const require = createRequire(import.meta.url);
const binding = require(resolve(process.argv[2]));
const cwd = mkdtempSync(join(tmpdir(), "zfb-native-css-"));
try {
  writeFileSync(join(cwd, "input.css"), ".authored { color: red; }");
  const result = JSON.parse(
    await binding.compileCssNative(
      JSON.stringify({ cwd, input: "input.css", config: { wind: false } }),
    ),
  );
  assert.equal(result.error, undefined, JSON.stringify(result));
  assert.match(result.result.css, /color: red/);
  assert.deepEqual(result.result.diagnostics, []);
  console.log("Native CSS load/compile PASS", process.platform, process.arch);
} finally {
  rmSync(cwd, { recursive: true, force: true });
}
