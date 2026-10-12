import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflow = readFileSync(join(root, ".github/workflows/island-size-darwin.yml"), "utf8");

describe("island-size-darwin workflow wiring", () => {
  it("is dispatch-only with no obsolete PR bootstrap", () => {
    expect(workflow).toContain("workflow_dispatch:");
    expect(workflow).not.toMatch(/^\s*pull_request:/m);
    expect(workflow).not.toContain("sweep-261007-2");
  });

  it("normalizes both measurements before the budget check", () => {
    const normalize = workflow.indexOf("normalize-identity.mjs");
    const check = workflow.indexOf("check-budget.mjs");
    expect(normalize).toBeGreaterThan(workflow.indexOf("measure-real.mjs"));
    expect(check).toBeGreaterThan(normalize);
    expect(workflow).toContain('--measurement "$out/real-$mode/measurement.json"');
    expect(workflow).toContain("for mode in workspace packed");
  });

  it("keeps the native guard and failure-time artifact upload", () => {
    expect(workflow).toContain('"Darwin:arm64"');
    expect(workflow).toMatch(/Upload complete Darwin island-size evidence\s+if: always\(\)/);
    expect(workflow).toContain("path: target/island-size/");
  });
});
