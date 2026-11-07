// Real CLI/adapter integration, using binaries built separately from this probe.
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import assert from "node:assert/strict";
const profile = process.env.ZFB_PROTOTYPE_PROFILE ?? "debug";
assert.ok(["debug", "release"].includes(profile));
const env = {
  ...process.env,
  ZFB_PROTOTYPE_CLI: process.env.ZFB_PROTOTYPE_CLI ?? resolve(`target/${profile}/zfb`),
};
// The zfb CLI rejects both retired backend selectors, so never forward them.
delete env.ZFB_ROLLDOWN_PROTOTYPE;
delete env.ZFB_BUNDLER;
const result = spawnSync(`target/${profile}/examples/rolldown_production`, [], {
  env,
  stdio: "inherit",
});
assert.equal(result.status, 0, "rolldown production probe failed");
const report = JSON.parse(
  readFileSync("target/rolldown-production/rolldown/evidence/report.json", "utf8"),
);
assert.equal(report.backend, "rolldown");
for (const route of ["/runtime", "/api/admin/a/b", "/api/admin/c", "/", "/throw"])
  assert.equal(report.scratch[route].status, report.adapter[route].status);
for (const route of ["/runtime", "/api/admin/a/b", "/api/admin/c"])
  assert.equal(report.scratch[route].body, report.adapter[route].body);
console.log(
  "Production probe: scratch and adapter response semantics agree; Wasm manifest and the explicit unsupported SSR copied-resource result are asserted by the probe itself.",
);
