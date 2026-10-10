// Small direct-Rust API/graph/output gate, independent of the ZFB pipelines.
import { mkdtempSync, writeFileSync, readFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import assert from "node:assert/strict";
const binary = resolve(process.env.ZFB_PREPARED_RUNNER ?? "target/debug/examples/prepared");
const root = mkdtempSync(resolve("target/rolldown-gate-"));
writeFileSync(
  join(root, "entry.mjs"),
  'export { answer } from "./dep.mjs"; export const later = () => import("./lazy.mjs");',
);
writeFileSync(join(root, "dep.mjs"), "export const answer = 42;");
writeFileSync(join(root, "lazy.mjs"), "export const answer = 43;");
const args = [
  "entry.mjs",
  "--bundle",
  "--format=esm",
  "--platform=neutral",
  "--splitting",
  "--outdir=out",
  "--sourcemap=linked",
  "--metafile=meta.json",
];
const result = spawnSync(binary, args, { cwd: root, encoding: "utf8" });
assert.equal(result.status, 0, result.stderr);
const meta = JSON.parse(readFileSync(join(root, "meta.json")));
assert.equal(Object.keys(meta.inputs).length, 3);
assert.ok(meta.inputs["entry.mjs"].imports.some((edge) => edge.path === "dep.mjs"));
assert.ok(
  meta.inputs["entry.mjs"].imports.some(
    (edge) => edge.path === "lazy.mjs" && edge.kind === "dynamic-import",
  ),
);
const entry = Object.entries(meta.outputs).find(([, value]) => value.entryPoint === "entry.mjs")[0];
const output = await import(pathToFileURL(entry));
assert.equal(output.answer, 42);
assert.equal((await output.later()).answer, 43);
assert.match(readFileSync(entry, "utf8"), /sourceMappingURL=/);
writeFileSync(join(root, "entry.mjs"), 'import "./absent.mjs";');
const missing = spawnSync(binary, args, { cwd: root, encoding: "utf8" });
assert.notEqual(missing.status, 0);
assert.match(missing.stderr, /absent.mjs/);
console.log(
  "direct native API: real static/dynamic graph, executed split output, linked map, actionable unresolved import PASS",
);
