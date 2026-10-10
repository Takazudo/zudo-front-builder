// Execute prepared binaries only; keep Cargo/tool installation outside timings.
import { spawnSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { resolve, join, dirname } from "node:path";
import { createHash } from "node:crypto";
import assert from "node:assert/strict";
const profile = process.env.ZFB_PROTOTYPE_PROFILE ?? "debug";
assert.ok(["debug", "release"].includes(profile));
const root = resolve(process.argv[2] ?? "target/rolldown-prototype/project");
function run(command, args, env = process.env) {
  const result = spawnSync(command, args, { env, stdio: "inherit" });
  assert.equal(result.status, 0, `${command} failed`);
}
run(process.execPath, ["prototypes/rolldown/gate.mjs"]);
for (const backend of ["esbuild", "rolldown"]) {
  const env = { ...process.env, ZFB_DEV_TIMING: "1" };
  delete env.ZFB_ROLLDOWN_PROTOTYPE;
  if (backend === "rolldown") env.ZFB_ROLLDOWN_PROTOTYPE = "1";
  run(`target/${profile}/examples/rolldown_ssr`, [root], env);
  const produce = () => run(`target/${profile}/examples/rolldown_browser`, [root], env);
  const snapshot = () => {
    const assets = join(dirname(root), `dist-${backend}`, "assets");
    return readdirSync(assets)
      .sort()
      .map((name) => [
        name,
        createHash("sha256")
          .update(readFileSync(join(assets, name)))
          .digest("hex"),
      ]);
  };
  produce();
  const previous = snapshot();
  produce(); // A new process and randomly named client stage.
  assert.deepEqual(snapshot(), previous, `${backend}: fresh-stage nondeterminism`);
  console.log(`${backend}: fresh-process/client-stage deterministic asset names and bytes PASS`);
}
run(process.execPath, ["prototypes/rolldown/browser.mjs", root]);
