import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

import {
  describeGzipChanges,
  diffCeilings,
  parseArgs,
  readmeSection,
  rebaselineContract,
  rewriteTestPins,
} from "../rebaseline-island-size-linux.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const contract = JSON.parse(
  readFileSync(join(root, "research/v3-island-size/decision-linux-x64.json"), "utf8"),
);
const testSource = readFileSync(
  join(root, "scripts/__tests__/island-size-budget.test.mjs"),
  "utf8",
);

function totalsFrom(base, edits = {}) {
  const totals = structuredClone(base.ceilings);
  for (const [path, value] of Object.entries(edits)) {
    const [mode, fixture] = path.split("/");
    totals[mode][fixture] = { ...totals[mode][fixture], ...value };
  }
  return totals;
}

const evidence = {
  version: "9.9.9",
  pnpmLockSha256: "a".repeat(64),
  sourceSha: "b".repeat(40),
  headSha: "c".repeat(40),
  runId: 1,
  jobId: 2,
  runLabel: "Release probe run for v9.9.9",
};

describe("rebaseline-island-size-linux", () => {
  it("records gzip-only movement as exact zero-allowance ceilings", () => {
    const totals = totalsFrom(contract, {
      "workspace/model": { gzip: contract.ceilings.workspace.model.gzip + 3 },
      "packed/json-api": { gzip: contract.ceilings.packed["json-api"].gzip - 2 },
    });
    const { contract: next, gzipChanges } = rebaselineContract(contract, totals, evidence);
    expect(next.ceilings).toEqual(totals);
    expect(next.allowance).toEqual({ raw: 0, gzip: 0 });
    expect(next.outcome).toBe("no-product-code-change");
    expect(next.baselineSourceSha).toBe(evidence.headSha);
    expect(next.toolchain.packageVersion).toBe("9.9.9");
    expect(next.toolchain.pnpmLockSha256).toBe(evidence.pnpmLockSha256);
    expect(next.baselineEvidence).toMatchObject({
      runId: 1,
      jobId: 2,
      sourceSha: evidence.sourceSha,
    });
    expect(next.baselineEvidence.note).toContain(
      "PR merge source " + evidence.sourceSha + " (probe head " + evidence.headSha + ")",
    );
    expect(next.platformStatus).toContain("2 gzip totals shifted by -2 to +3 bytes");
    expect(describeGzipChanges(gzipChanges)).toBe("workspace model +3; packed json-api -2");
    expect(contract.toolchain.packageVersion).not.toBe("9.9.9");
  });

  it("refuses a raw change, which is a product size decision rather than a version stamp", () => {
    const totals = totalsFrom(contract, {
      "packed/show-for": { raw: contract.ceilings.packed["show-for"].raw + 1 },
    });
    expect(diffCeilings(contract, totals).rawChanges).toHaveLength(1);
    expect(() => rebaselineContract(contract, totals, evidence)).toThrow(/raw totals changed/);
  });

  it("refuses a missing fixture", () => {
    const totals = totalsFrom(contract);
    delete totals.workspace["blog-theme"];
    expect(() => rebaselineContract(contract, totals, evidence)).toThrow(
      /fixture set differs from the contract \(workspace\/blog-theme\)/,
    );
  });

  it("names a push run's source plainly when it measured the head itself", () => {
    const { contract: next } = rebaselineContract(contract, totalsFrom(contract), {
      ...evidence,
      sourceSha: evidence.headSha,
    });
    expect(next.platformStatus).toContain("at source " + evidence.headSha + ".");
    expect(next.platformStatus).toContain("no gzip total changed");
  });

  it("validates CLI arguments before touching any file", () => {
    const ok = [
      "--artifact",
      "dir",
      "--run-id",
      "12",
      "--job-id",
      "34",
      "--head-sha",
      "d".repeat(40),
    ];
    expect(parseArgs(ok)).toMatchObject({ "run-id": "12", "head-sha": "d".repeat(40) });
    expect(() => parseArgs(ok.slice(0, 6))).toThrow(/required --head-sha/);
    expect(() => parseArgs(ok.map((v) => (v === "12" ? "12a" : v)))).toThrow(/--run-id must be/);
    expect(() => parseArgs(ok.map((v) => (v === "d".repeat(40) ? "abc123" : v)))).toThrow(
      /--head-sha must be/,
    );
    expect(() => parseArgs(["--artifact"])).toThrow(/bad argument/);
  });

  it("rewrites the Linux ceiling test pins and leaves the rest of the file alone", () => {
    const totals = totalsFrom(contract, {
      "workspace/event-only": { gzip: 11111 },
      "packed/no-island": { gzip: 0 },
    });
    const { contract: next } = rebaselineContract(contract, totals, evidence);
    const rewritten = rewriteTestPins(testSource, next);
    expect(rewritten).toContain('toolchain.packageVersion).toBe("9.9.9")');
    expect(rewritten).toContain(
      '"event-only": { raw: ' + totals.workspace["event-only"].raw + ", gzip: 11111 },",
    );
    const outside = (s) => s.slice(0, s.indexOf('it("records the reviewed Linux x64 ceilings'));
    expect(outside(rewritten)).toBe(outside(testSource));
    expect(rewriteTestPins(testSource, contract)).toBe(testSource);
  });

  it("writes a README section naming the run, source and moved rows", () => {
    const totals = totalsFrom(contract, {
      "workspace/model": { gzip: contract.ceilings.workspace.model.gzip + 1 },
    });
    const {
      contract: next,
      gzipChanges,
      previousVersion,
    } = rebaselineContract(contract, totals, evidence);
    const section = readmeSection(next, gzipChanges, previousVersion);
    expect(section).toContain("## v9.9.9 release baseline");
    expect(section).toContain("actions/runs/1) (job 2)");
    expect(section).toContain("moved 1 gzip totals by +1 to +1 bytes (workspace model +1)");
  });
});
