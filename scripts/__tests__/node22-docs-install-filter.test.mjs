// The docs member's @takazudo/zudo-doc declares engines.node ">=24 <25" and
// pnpm-workspace.yaml sets engineStrict, so an unfiltered workspace install
// fails on Node 22 (#4433). Workspace jobs run on Node 24; the few that still
// run on Node 22 to cover the published floor must leave docs out.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflowsDir = join(rootDir, ".github/workflows");
const DOCS_EXCLUDED = /--filter\s+(?:'!docs'|"!docs"|zudo-front-builder\b)/;

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
  it("finds the Node 22 package-floor lane", () => {
    expect(node22Installs().map(({ where }) => where)).toContain("health.yml node22-packages");
  });

  it("exclude the Node 24-only docs member", () => {
    const unfiltered = node22Installs()
      .filter(({ run }) => !DOCS_EXCLUDED.test(run))
      .map(({ where }) => where);
    expect(unfiltered).toEqual([]);
  });
});
