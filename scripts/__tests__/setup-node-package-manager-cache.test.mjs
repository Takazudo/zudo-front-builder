// actions/setup-node v5 caches the root packageManager (pnpm) by default, and
// fails a job that never installs pnpm. Release run 43375067743 (v6.1.1) lost
// its OIDC publish job this way, before anything reached npm.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";
import { scriptsDir } from "./helpers/darwin-fixture.mjs";

const workflowsDir = join(scriptsDir, "..", ".github", "workflows");

function pnpmlessSetupNodeSteps() {
  const offenders = [];
  for (const file of readdirSync(workflowsDir).filter((f) => /\.ya?ml$/.test(f))) {
    const workflow = parse(readFileSync(join(workflowsDir, file), "utf8"));
    for (const [jobName, job] of Object.entries(workflow.jobs ?? {})) {
      let pnpmInstalled = false;
      for (const step of job.steps ?? []) {
        const uses = String(step.uses ?? "");
        if (uses.startsWith("pnpm/action-setup@")) pnpmInstalled = true;
        if (
          uses.startsWith("actions/setup-node@") &&
          !pnpmInstalled &&
          step.with?.["package-manager-cache"] !== false
        ) {
          offenders.push(`${file} ${jobName}`);
        }
      }
    }
  }
  return offenders;
}

describe("setup-node without pnpm", () => {
  it("opts out of package-manager caching in every job that has not installed pnpm", () => {
    expect(pnpmlessSetupNodeSteps()).toEqual([]);
  });
});
