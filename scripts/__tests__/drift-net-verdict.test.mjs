import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";
import { DRIFT_NET_JOBS, driftNetVerdict } from "../drift-net-verdict.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflow = readFileSync(join(root, ".github/workflows/drift-net.yml"), "utf8");

const all = (result) => Object.fromEntries(DRIFT_NET_JOBS.map((job) => [job, result]));
const verdict = (results, legs = "all") => driftNetVerdict({ legs, results });

function jobBlock(name) {
  const start = workflow.indexOf(`\n  ${name}:\n`);
  expect(start).toBeGreaterThan(-1);
  const next = workflow.slice(start + 1).search(/\n {2}[a-z][a-z-]*:\n/);
  return next < 0 ? workflow.slice(start) : workflow.slice(start, start + 1 + next);
}

describe("drift-net issue truth table (#4533)", () => {
  it("covers exactly the five jobs", () => {
    expect(DRIFT_NET_JOBS).toHaveLength(5);
  });

  it("closes only when all five jobs succeeded", () => {
    expect(verdict(all("success"))).toEqual({ action: "close", failed: [] });
    expect(verdict(all("success"), "")).toEqual({ action: "close", failed: [] });
  });

  it.each(["failure", "cancelled"])("files on %s in any single job and names it", (result) => {
    for (const job of DRIFT_NET_JOBS) {
      expect(verdict({ ...all("success"), [job]: result })).toEqual({
        action: "file",
        failed: [job],
      });
    }
  });

  it("never closes when any job is skipped or otherwise not success", () => {
    for (const job of DRIFT_NET_JOBS) {
      for (const result of ["skipped", "", undefined]) {
        expect(verdict({ ...all("success"), [job]: result }).action).toBe("none");
      }
    }
    expect(verdict(all("skipped")).action).toBe("none");
  });

  it("still files when a failure sits beside skipped jobs", () => {
    const results = { ...all("skipped"), "provenance-latest": "failure" };
    expect(verdict(results)).toEqual({ action: "file", failed: ["provenance-latest"] });
  });

  it("a preset-carrier dispatch writes nothing, even when red or green", () => {
    expect(verdict(all("success"), "preset-carrier").action).toBe("none");
    expect(verdict(all("failure"), "preset-carrier")).toEqual({ action: "none", failed: [] });
  });
});

describe("drift-net.yml wiring (#4533)", () => {
  it("pins ZFB_PRESET_BINARY=installed, hard-fail, 14-day evidence on both preset jobs", () => {
    for (const name of ["preset-installed-carrier-ubuntu", "preset-installed-carrier-windows"]) {
      const block = jobBlock(name);
      expect(block).toMatch(/^ {6}ZFB_PRESET_BINARY: installed$/m);
      expect(block).toContain("run: node tests/published-preset-identity/run.mjs");
      expect(block).not.toContain("continue-on-error");
      expect(block).toMatch(/retention-days: 14/);
      expect(block).toMatch(/if: always\(\)/);
    }
    expect(jobBlock("preset-installed-carrier-ubuntu")).toContain("runs-on: ubuntu-latest");
    expect(jobBlock("preset-installed-carrier-ubuntu")).toContain(
      "playwright install --with-deps chromium",
    );
    expect(jobBlock("preset-installed-carrier-windows")).toContain("runs-on: windows-latest");
    expect(jobBlock("preset-installed-carrier-windows")).toMatch(/playwright install chromium$/m);
  });

  it("has no macOS preset job", () => {
    expect(workflow).not.toMatch(/preset-installed-carrier-macos/);
  });

  it("keys concurrency by selection and skips the old legs on preset-carrier", () => {
    expect(workflow).toContain(
      "group: drift-net-${{ github.ref }}-${{ github.event.inputs.legs || 'all' }}",
    );
    for (const name of ["smoke-latest-ubuntu", "smoke-latest-macos", "provenance-latest"]) {
      expect(jobBlock(name)).toContain("if: ${{ github.event.inputs.legs != 'preset-carrier' }}");
    }
  });

  it("makes the issue and notify jobs depend on all five and skip on preset-carrier", () => {
    for (const name of ["file-or-close-issue", "notify"]) {
      const block = jobBlock(name);
      for (const job of DRIFT_NET_JOBS) expect(block).toContain(`- ${job}\n`);
      expect(block).toContain("!cancelled() && github.event.inputs.legs != 'preset-carrier'");
      expect(block).toContain("drift-net-verdict.mjs");
    }
  });

  it("no longer claims everything smokes latest", () => {
    expect(workflow).not.toContain("Channel coverage — `latest` only; see");
    expect(workflow).toContain("PINNED published tuple");
  });
});
