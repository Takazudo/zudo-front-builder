// Run in a clean room containing the packed SDK; platform is simulated, not native.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { mkdirSync, writeFileSync, existsSync } from "node:fs";
const require = createRequire(import.meta.url);
const manifest = require("@takazudo/zfb/package.json");
assert.equal(manifest.optionalDependencies?.["@takazudo/zfb-linux-arm64-gnu"], undefined);
const sdk = dirname(require.resolve("@takazudo/zfb/package.json"));
const stale = join(sdk, "node_modules/@takazudo/zfb-linux-arm64-gnu");
mkdirSync(stale, { recursive: true });
writeFileSync(
  join(stale, "package.json"),
  JSON.stringify({ name: "@takazudo/zfb-linux-arm64-gnu", version: "6.0.1" }),
);
const marker = join(stale, "executed");
writeFileSync(join(stale, "zfb"), `#!/bin/sh\ntouch '${marker}'\n`, { mode: 0o755 });
const preload = join(sdk, "simulate-arm64.mjs");
writeFileSync(
  preload,
  'Object.defineProperty(process,"platform",{value:"linux"});Object.defineProperty(process,"arch",{value:"arm64"});',
);
const cli = spawnSync(
  process.execPath,
  ["--import", preload, join(sdk, "bin/zfb.mjs"), "--version"],
  { encoding: "utf8" },
);
assert.equal(cli.status, 1);
assert.match(cli.stderr, /Linux arm64.*suspended/);
assert.doesNotMatch(cli.stderr, /reinstall|optional dependencies/i);
assert.equal(existsSync(marker), false);
// When create-zfb is installed, prove that its inherited stderr explains the cause.
try {
  const generator = dirname(require.resolve("create-zfb/package.json"));
  const generated = spawnSync(
    process.execPath,
    [join(generator, "bin/create-zfb.mjs"), "example"],
    {
      encoding: "utf8",
      env: { ...process.env, NODE_OPTIONS: `--import=${preload}` },
    },
  );
  assert.equal(generated.status, 1);
  assert.match(generated.stderr, /Linux arm64.*suspended/);
} catch (error) {
  if (error.code !== "MODULE_NOT_FOUND" && error.code !== "ERR_PACKAGE_PATH_NOT_EXPORTED")
    throw error;
}
Object.defineProperty(process, "platform", { value: "linux" });
Object.defineProperty(process, "arch", { value: "arm64" });
await import("@takazudo/zfb/paginate");
const { compileCss } = await import("@takazudo/zfb/css");
await assert.rejects(compileCss({ input: "input.css" }), /Linux arm64.*suspended/);
console.log(
  "passed: packed SDK simulated Linux arm64 lazy import, CLI stale-carrier refusal, CSS suspension",
);
