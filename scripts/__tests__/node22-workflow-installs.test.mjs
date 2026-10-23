// The root and the published packages require Node 24, and the docs member's
// @takazudo/zudo-doc does too under engineStrict. A workflow install on
// Node 22 would fail or test an unsupported runtime; this guards new jobs.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflowsDir = join(rootDir, ".github/workflows");

const node22Installs = () => {
  const found = [];
  for (const file of readdirSync(workflowsDir).filter((name) => name.endsWith(".yml"))) {
    const workflow = parse(readFileSync(join(workflowsDir, file), "utf8"));
    for (const [jobName, job] of Object.entries(workflow?.jobs ?? {})) {
      let node;
      for (const step of job.steps ?? []) {
        if (String(step.uses ?? "").startsWith("actions/setup-node@")) {
          node = String(step.with?.["node-version"] ?? "");
        }
        const run = String(step.run ?? "");
        if (/^22(\.|$)/.test(node ?? "") && /\bpnpm install\b/.test(run)) {
          found.push({ where: `${file} ${jobName}`, run });
        }
      }
    }
  }
  return found;
};

describe("Node 22 workflow installs (#4433)", () => {
  it("are gone: the workspace and the published packages require Node 24", () => {
    expect(node22Installs().map(({ where }) => where)).toEqual([]);
  });
});
