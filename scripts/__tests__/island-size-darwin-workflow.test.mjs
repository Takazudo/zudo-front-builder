import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflow = readFileSync(join(root, ".github/workflows/island-size-darwin.yml"), "utf8");

// Each top-level job's text, from its `  <name>:` line to the next job's, so an
// assertion on one job can never be satisfied by a later one.
const jobsText = workflow.slice(workflow.indexOf("\njobs:\n"));
const jobNames = [...jobsText.matchAll(/^ {2}([a-z][a-z0-9-]*):$/gm)].map((match) => match[1]);
function jobText(name) {
  const start = jobsText.search(new RegExp("^ {2}" + name + ":$", "m"));
  const next = jobNames[jobNames.indexOf(name) + 1];
  const end = next ? jobsText.search(new RegExp("^ {2}" + next + ":$", "m")) : jobsText.length;
  return jobsText.slice(start, end);
}
const darwin = jobText("measure-darwin");
const linux = jobText("measure-linux");
const carriers = jobText("css-carriers");

function expectOrdered(job) {
  const measure = job.indexOf("measure-real.mjs");
  const normalize = job.indexOf("normalize-identity.mjs");
  const check = job.indexOf("check-budget.mjs");
  expect(measure).toBeGreaterThan(-1);
  expect(normalize).toBeGreaterThan(measure);
  expect(check).toBeGreaterThan(normalize);
  expect(job).toContain('--measurement "$out/real-$mode/measurement.json"');
  expect(job).toContain("for mode in workspace packed");
}

describe("island-size native workflow wiring", () => {
  it("is dispatch-only with no obsolete PR bootstrap", () => {
    expect(workflow).toContain("workflow_dispatch:");
    expect(workflow).not.toMatch(/^\s*pull_request:/m);
    expect(workflow).not.toMatch(/^\s*push:/m);
    expect(workflow).not.toContain("sweep-261007-2");
  });

  it("keeps the registered filename and displays as island-size-native", () => {
    expect(workflow).toMatch(/^name: island-size-native$/m);
  });

  it("declares the platform choice input defaulting to darwin-arm64", () => {
    expect(workflow).toMatch(
      /platform:\n\s+description: .+\n\s+required: false\n\s+default: darwin-arm64\n\s+type: choice\n\s+options:\n\s+- darwin-arm64\n\s+- linux-x64\n\s+- both\n/,
    );
  });

  it("declares exactly the two measure jobs and the CSS carrier lane, in order", () => {
    expect(jobNames).toEqual(["measure-darwin", "measure-linux", "css-carriers"]);
    expect(linux).not.toContain("css-carrier");
    expect(darwin).not.toContain("css-carrier");
  });

  it("gates each job on inputs.platform", () => {
    expect(darwin).toContain("if: inputs.platform == 'darwin-arm64' || inputs.platform == 'both'");
    expect(linux).toContain("if: inputs.platform == 'linux-x64' || inputs.platform == 'both'");
    expect(workflow).not.toMatch(/^ {2}measure:/m);
  });

  it("normalizes both measurements before the budget check in each job", () => {
    expectOrdered(darwin);
    expectOrdered(linux);
  });

  it("keeps the Darwin native guard, artifact name and failure-time upload", () => {
    expect(darwin).toContain('"Darwin:arm64"');
    expect(darwin).toContain(
      "name: island-size-darwin-arm64-${{ steps.context.outputs.source_sha }}",
    );
    expect(darwin).toMatch(/Upload complete Darwin island-size evidence\s+if: always\(\)/);
    expect(darwin).toContain("path: target/island-size/");
    expect(darwin).not.toContain("--contract");
  });

  it("runs the Linux lane natively against the Linux contract", () => {
    expect(linux).toContain("runs-on: ubuntu-24.04");
    expect(linux).toContain('"Linux:x86_64"');
    expect(linux).toContain('process.platform !== "linux" || process.arch !== "x64"');
    expect(linux).toContain("--contract research/v3-island-size/decision-linux-x64.json");
    expect(linux).toContain('prefix-key: "v1-island-size-linux"');
    expect(linux).not.toContain("shared-key");
    expect(linux).not.toContain("test:workspace");
    expect(linux).not.toContain("Darwin");
  });

  it("uploads Linux evidence on failure too", () => {
    expect(linux).toMatch(/Upload complete Linux x64 island-size evidence\s+if: always\(\)/);
    expect(linux).toContain("name: island-size-linux-x64-${{ steps.context.outputs.source_sha }}");
    expect(linux).toContain("path: target/island-size/");
  });

  it("keeps the CSS carrier lane independent of the platform measure jobs", () => {
    for (const input of ["css_carriers", "css_windows_only"]) {
      expect(workflow).toMatch(
        new RegExp(
          "\\n {6}" +
            input +
            ":\\n\\s+description: .+\\n\\s+required: false\\n\\s+default: false\\n\\s+type: boolean\\n",
        ),
      );
    }
    expect(carriers).toContain("if: ${{ inputs.css_carriers }}");
    expect(carriers).not.toContain("inputs.platform");
    expect(carriers).toContain("uses: ./.github/workflows/css-carrier-verification.yml");
    expect(carriers).toContain("source_sha: ${{ inputs.ref }}");
    expect(carriers).toContain("windows_only: ${{ inputs.css_windows_only }}");
    expect(carriers).not.toContain("check-budget.mjs");
  });
});
