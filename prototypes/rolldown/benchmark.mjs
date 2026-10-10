// Bounded release experiment; build separately, then run under the heavy guard.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { arch, cpus, platform, release } from "node:os";
const root = resolve("target/rolldown-prototype/project");
const destination = resolve("target/rolldown-release-measurement");
mkdirSync(destination, { recursive: true });
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
function tree(directory) {
  const files = [];
  function visit(path, relative = "") {
    for (const entry of readdirSync(path, { withFileTypes: true }).sort((a, b) =>
      a.name.localeCompare(b.name),
    )) {
      const name = join(relative, entry.name);
      // Copied SDK packages carry development-install links; selected runtime
      // dependencies are materialised separately at the fixture top level.
      if (entry.name === "node_modules" && relative !== "") continue;
      if (entry.isDirectory()) visit(join(path, entry.name), name);
      else if (entry.isFile()) files.push([name, hash(readFileSync(join(path, entry.name)))]);
      else assert.fail(`Input snapshot requires regular file/directory: ${path}/${entry.name}`);
    }
  }
  visit(directory);
  return { sha256: hash(JSON.stringify(files)), files };
}
function run(command, args, env, label) {
  const start = process.hrtime.bigint();
  const out = spawnSync(command, args, { env, encoding: "utf8", maxBuffer: 32 * 1024 * 1024 });
  const elapsed_us = Number((process.hrtime.bigint() - start) / 1000n);
  const log = `${out.stdout ?? ""}\n${out.stderr ?? ""}`;
  writeFileSync(join(destination, `${label}.log`), log);
  assert.equal(out.status, 0, `${label}: ${log}`);
  return { elapsed_us, log };
}
const base = {
  ...process.env,
  ZFB_DEV_TIMING: "1",
  ZFB_PROTOTYPE_PROFILE: "release",
  ZFB_PROTOTYPE_CLI: resolve("target/release/zfb"),
};
delete base.ZFB_ROLLDOWN_PROTOTYPE;
const before = tree(root);
const productionInputs = tree("prototypes/rolldown/production-fixture");
const samples = [];
// AB / BA counters order bias without claiming a statistical speed result.
for (const [round, order] of [
  [0, ["esbuild", "rolldown"]],
  [1, ["rolldown", "esbuild"]],
]) {
  for (const backend of order) {
    const env = { ...base };
    if (backend === "rolldown") env.ZFB_ROLLDOWN_PROTOTYPE = "1";
    for (const fixture of ["ssr", "browser", "production"]) {
      const label = `${round}-${backend}-${fixture}`;
      const { elapsed_us, log } = run(
        `target/release/examples/rolldown_${fixture}`,
        fixture === "production" ? [] : [root],
        env,
        label,
      );
      const record = { round, backend, fixture, probe_process_us: elapsed_us };
      if (fixture === "production") {
        const evidence = `target/rolldown-production/${backend}/evidence`;
        cpSync(evidence, join(destination, label), { recursive: true });
        record.positive_cli_calls = [0, 1].map((iteration) => {
          const timing = JSON.parse(readFileSync(`${evidence}/positive-${iteration}-timing.json`));
          const positiveLog = readFileSync(`${evidence}/positive-${iteration}.log`, "utf8");
          return {
            ...timing,
            phase_lines: positiveLog
              .split("\n")
              .filter((line) => line.includes("bundle():") || line.includes("rolldown-prototype:")),
          };
        });
      } else {
        const key = fixture === "ssr" ? "selected_pipeline_us" : "selected_bundle_call_us";
        const values = log.match(new RegExp(`${key}=\\[([^\\]]+)\\]`));
        assert.ok(values, `missing ${key}`);
        record[key] = values[1].split(",").map(Number);
        assert.equal(record[key].length, 3);
        // SSR negative probes follow the first three positive calls. Never pool
        // their timings into the measured positive sample set.
        record.positive_pipeline_phase_lines = log
          .split("\n")
          .filter((line) => line.includes("bundle():"))
          .slice(0, fixture === "ssr" ? 3 : 0);
        const native = [
          ...log.matchAll(/rolldown-prototype: bundle=(\d+)us metadata=(\d+)us/g),
        ].map((match) => ({ bundle_us: Number(match[1]), metadata_us: Number(match[2]) }));
        if (backend === "rolldown") {
          const jobsPerCall = fixture === "browser" ? 2 : 1; // main + module worker
          assert.ok(native.length >= 3 * jobsPerCall);
          record.positive_native_jobs_by_call = Array.from({ length: 3 }, (_, index) =>
            native.slice(index * jobsPerCall, (index + 1) * jobsPerCall),
          );
        }
        if (fixture === "browser") {
          const publish = log.match(/production_publish_us=\[([^\]]+)\]/);
          assert.ok(publish);
          record.production_publish_us = publish[1].split(",").map(Number);
        }
      }
      samples.push(record);
      console.log(
        `${label}: semantic probe PASS (${elapsed_us}us entire probe, not a bundle metric)`,
      );
    }
  }
}
assert.deepEqual(tree(root), before, "prepared input tree changed during comparison");
assert.deepEqual(tree("prototypes/rolldown/production-fixture"), productionInputs);
// Final emitted browser assets execute in Chromium; production V8 checks ran per sample.
run(process.execPath, ["prototypes/rolldown/browser.mjs", root], base, "chromium");
run(process.execPath, ["prototypes/rolldown/gate.mjs"], base, "direct-gate");
for (const round of [0, 1]) {
  const reports = ["esbuild", "rolldown"].map((backend) =>
    JSON.parse(readFileSync(join(destination, `${round}-${backend}-production/report.json`))),
  );
  for (const route of ["/runtime", "/api/admin/a/b", "/api/admin/c"])
    assert.equal(reports[0].adapter[route].body, reports[1].adapter[route].body);
  assert.deepEqual(reports[0].wasm_values, reports[1].wasm_values);
}
const git = spawnSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" });
const report = {
  source_sha: git.stdout.trim(),
  worktree_status: spawnSync("git", ["status", "--porcelain"], { encoding: "utf8" }).stdout,
  dirty_diff_sha256: hash(spawnSync("git", ["diff", "HEAD", "--"], { encoding: "utf8" }).stdout),
  input_snapshot_scope:
    "Materialised fixture sources and top-level copied runtime dependencies, excluding nested package node_modules development installs. This is not a byte-for-byte prepared argv/stage hash; both backends use the same existing preparation/policy code.",
  platform: platform(),
  arch: arch(),
  kernel: release(),
  cpu: cpus()[0]?.model,
  node: process.version,
  profile: "Cargo release, unchanged workspace settings",
  interpretation:
    "Two fresh probe processes/backend; each SSR/browser has three calls (first then two same-process warm calls, no bundler instance reuse). Production samples each spawn a fresh CLI; second has an existing dist. No OS cache flush. Compilation excluded. Native internal phases available; esbuild internal metadata phase unavailable. Probe process time includes validation/negative cases and is not pipeline time.",
  prepared_inputs: before,
  production_inputs: productionInputs,
  binaries: Object.fromEntries(
    [
      "target/release/zfb",
      "target/release/examples/rolldown_ssr",
      "target/release/examples/rolldown_browser",
      "target/release/examples/rolldown_production",
      process.env.ZFB_ESBUILD_BIN,
    ].map((path) => {
      assert.ok(path);
      return [path, hash(readFileSync(path))];
    }),
  ),
  samples,
};
writeFileSync(join(destination, "report.json"), JSON.stringify(report, null, 2) + "\n");
console.log(`Release measurement and semantic gates PASS: ${destination}/report.json`);
