// Contract for skipping heavy PR lanes on docs-only pull requests (#4372).
// A required context that is skipped by a job-level `if:` reports success, so
// every skip here must be fail-closed and the docs-only PR must still run
// health's JS/format subset behind the required `Docs gate`.
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const FILTER_FILE = ".github/path-filters/docs-only.yml";
const DOCS_ONLY_SKIP = "!cancelled() && needs.changed-files.outputs.docs_only != 'true'";

const readWorkflow = (name) =>
  parse(readFileSync(join(rootDir, ".github/workflows", name), "utf8"));
const health = readWorkflow("health.yml");
const smoke = readWorkflow("node-free-smoke.yml");
const docsChecks = readWorkflow("docs-checks.yml");
const filter = parse(readFileSync(join(rootDir, FILTER_FILE), "utf8"));

const bareIf = (job) =>
  String(job.if ?? "")
    .replace(/^\$\{\{\s*/, "")
    .replace(/\s*\}\}$/, "");

function detectorSteps(job) {
  return job.steps.filter((step) => step.with?.filters === FILTER_FILE);
}

describe("docs-only path filter", () => {
  it("is one positive catch-all followed only by negated prose paths", () => {
    expect(Object.keys(filter)).toEqual(["code"]);
    const [first, ...rest] = filter.code;
    expect(first).toBe("**");
    expect(rest.length).toBeGreaterThan(0);
    for (const pattern of rest) expect(pattern.startsWith("!")).toBe(true);
  });

  it("never treats Markdown test inputs as docs-only", () => {
    // research/3242-zudo-wind-v1-spec.md is an include_str! input of a Rust
    // test, and crates/zfb/templates/** is embedded into the binary.
    for (const pattern of filter.code.slice(1)) {
      expect(pattern).not.toMatch(/^!research\b/);
      expect(pattern).not.toMatch(/^!crates\/\*\*/);
      expect(pattern).not.toMatch(/^!\*\*\//);
      expect(pattern).not.toContain("templates");
      expect(pattern).not.toContain("tests");
      expect(pattern).not.toContain("fixtures");
    }
  });

  it("is read by every detector with predicate-quantifier every", () => {
    const detectors = [
      health.jobs["changed-files"],
      smoke.jobs.changes,
      docsChecks.jobs["changed-files"],
    ];
    for (const job of detectors) {
      const steps = detectorSteps(job);
      expect(steps).toHaveLength(1);
      expect(steps[0].with["predicate-quantifier"]).toBe("every");
      expect(job.outputs.docs_only).toBe("${{ steps.scope.outputs.docs_only }}");
      const scope = job.steps.find((step) => step.id === "scope");
      expect(scope.run).toContain('[ "$CODE_CHANGED" = "false" ]');
    }
    expect(health.jobs["changed-files"].steps.find((step) => step.id === "scope").run).toContain(
      '[ "$EVENT_NAME" = "pull_request" ]',
    );
  });
});

describe("health.yml docs-only skip", () => {
  it("skips the required jobs only on an explicit docs_only, never on detector failure", () => {
    for (const name of ["health", "build-no-v8"]) {
      const job = health.jobs[name];
      expect(job.needs).toBe("changed-files");
      expect(bareIf(job)).toBe(DOCS_ONLY_SKIP);
    }
  });

  it("keeps the optional md-wasm jobs off docs-only PRs", () => {
    for (const name of ["wasm-md", "wasm-md-browser"]) {
      expect(bareIf(health.jobs[name])).toContain(
        "needs.changed-files.outputs.docs_only != 'true'",
      );
    }
  });
});

describe("node-free-smoke.yml docs-only skip", () => {
  it("keeps the matrix build job running so per-target contexts still report", () => {
    const job = smoke.jobs["build-binary"];
    expect(job.needs).toBe("changes");
    expect(bareIf(job)).toMatch(/^!cancelled\(\) && /);
    expect(bareIf(job)).not.toContain("docs_only");
    expect(job.env.BUILD_BINARY).toBe("${{ needs.changes.outputs.docs_only != 'true' }}");

    const [notice, ...steps] = job.steps;
    expect(notice.if).toBe("${{ env.BUILD_BINARY != 'true' }}");
    expect(steps.length).toBeGreaterThan(0);
    for (const step of steps) expect(step.if).toBe("${{ env.BUILD_BINARY == 'true' }}");
  });

  it("skips binary consumers only on an explicit docs_only after a successful build", () => {
    const consumers = Object.entries(smoke.jobs).filter(([, job]) =>
      [job.needs].flat().includes("build-binary"),
    );
    expect(consumers.map(([name]) => name).sort()).toEqual(
      [
        "built-site-browser-smoke",
        "scaffold-e2e",
        "smoke-amd64",
        "smoke-amd64-al2023",
        "smoke-amd64-ts",
        "smoke-arm64",
        "smoke-arm64-ts",
      ].sort(),
    );
    for (const [, job] of consumers) {
      expect(job.needs).toEqual(["build-binary", "changes"]);
      expect(bareIf(job)).toBe(
        "!cancelled() && needs.build-binary.result == 'success' && needs.changes.outputs.docs_only != 'true'",
      );
    }
  });
});

describe("docs-checks.yml docs-only lane", () => {
  it("runs health's commands and is enforced by the required Docs gate", () => {
    const lane = docsChecks.jobs["docs-only-checks"];
    expect(bareIf(lane)).toBe("needs.changed-files.outputs.docs_only == 'true'");

    const healthRuns = health.jobs.health.steps.map((step) => step.run).filter(Boolean);
    const laneRuns = lane.steps.map((step) => step.run).filter(Boolean);
    for (const command of [
      "pnpm typecheck:workspace",
      "pnpm test:workspace",
      "pnpm format:check",
      "node scripts/assert-md-wasm-size-docs.mjs",
      "node scripts/check-sse-endpoint.mjs",
    ]) {
      expect(laneRuns).toContain(command);
      expect(healthRuns).toContain(command);
    }
    for (const run of laneRuns) expect(healthRuns).toContain(run);

    const gate = docsChecks.jobs["docs-gate"];
    expect(gate.needs).toContain("docs-only-checks");
    expect(bareIf(gate)).toBe("always()");
    const verdict = gate.steps[0];
    expect(verdict.env.DOCS_ONLY_RESULT).toBe("${{ needs.docs-only-checks.result }}");
    expect(verdict.run).toContain('if [ "${DOCS_ONLY}" != "false" ]; then');
  });
});
