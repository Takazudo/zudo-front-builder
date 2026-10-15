// Real CLI/adapter integration, using binaries built separately from this probe.
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import assert from "node:assert/strict";
const reports = [];
const profile = process.env.ZFB_PROTOTYPE_PROFILE ?? "debug";
assert.ok(["debug", "release"].includes(profile));
for (const backend of ["esbuild", "rolldown"]) {
  const env = {
    ...process.env,
    ZFB_PROTOTYPE_CLI: process.env.ZFB_PROTOTYPE_CLI ?? resolve(`target/${profile}/zfb`),
  };
  delete env.ZFB_ROLLDOWN_PROTOTYPE;
  if (backend === "rolldown") env.ZFB_ROLLDOWN_PROTOTYPE = "1";
  const result = spawnSync(`target/${profile}/examples/rolldown_production`, [], {
    env,
    stdio: "inherit",
  });
  assert.equal(result.status, 0, `${backend} production probe failed`);
  reports.push(
    JSON.parse(readFileSync(`target/rolldown-production/${backend}/evidence/report.json`, "utf8")),
  );
}
for (const report of reports) {
  for (const route of ["/runtime", "/api/admin/a/b", "/api/admin/c", "/", "/throw"])
    assert.equal(report.scratch[route].status, report.adapter[route].status);
  for (const route of ["/runtime", "/api/admin/a/b", "/api/admin/c"])
    assert.equal(report.scratch[route].body, report.adapter[route].body);
}
for (const route of ["/runtime", "/api/admin/a/b", "/api/admin/c"])
  assert.equal(reports[0].adapter[route].body, reports[1].adapter[route].body);
assert.deepEqual(reports[0].wasm_values, reports[1].wasm_values);
assert.equal(reports[0].glue_adapter_handoff, reports[1].glue_adapter_handoff);
console.log(
  "Equivalent production inputs: response semantics, Wasm manifest, and explicit unsupported SSR copied-resource result agree across backends.",
);
