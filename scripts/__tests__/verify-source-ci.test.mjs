// scripts/release/verify-source-ci.mjs: required CI of one exact SHA (#4690).
// In-process with an injected `gh`; nothing reaches GitHub.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";
import {
  REQUIRED_CONTEXTS,
  classifyContext,
  evaluate,
  main,
  parseArgs,
} from "../release/verify-source-ci.mjs";
import { scriptsDir } from "./helpers/darwin-fixture.mjs";

const SHA = "b".repeat(40);
const repoRoot = join(scriptsDir, "..");

let nextId = 1;
const WORKFLOWS = [...new Set(REQUIRED_CONTEXTS.map((c) => c.workflow))];
const suiteOf = (workflow) => 1000 + WORKFLOWS.indexOf(workflow);
const workflowOf = (name) => REQUIRED_CONTEXTS.find((c) => c.context === name)?.workflow;
// A check run as readSourceCi returns it: suite resolved to its workflow file.
const run = (name, status = "completed", conclusion = "success", id = nextId++) => ({
  id,
  name,
  status,
  conclusion: status === "completed" ? conclusion : null,
  app: "github-actions",
  suite: suiteOf(workflowOf(name)),
  workflowPath: `.github/workflows/${workflowOf(name)}`,
});
const pushContexts = REQUIRED_CONTEXTS.filter((c) => !c.prOnly).map((c) => c.context);
const allGreen = () => pushContexts.map((name) => run(name));

describe("evaluate", () => {
  it("passes when every push-applicable context succeeded (pr-only reported, not counted)", () => {
    const verdict = evaluate({ sha: SHA, checkRuns: allGreen(), statuses: [] });
    expect(verdict.passed).toBe(true);
    const prOnly = verdict.results.filter((r) => r.prOnly);
    expect(prOnly.map((r) => r.state)).toEqual([
      "not-applicable-pr-only",
      "not-applicable-pr-only",
    ]);
    expect(verdict.results.filter((r) => r.state === "passed")).toHaveLength(pushContexts.length);
  });

  it("does not pass a skipped required job on a push source commit", () => {
    const runs = allGreen();
    runs[0].conclusion = "skipped";
    const verdict = evaluate({ sha: SHA, checkRuns: runs, statuses: [] });
    expect(verdict.passed).toBe(false);
    expect(verdict.blocking).toContain("health: skipped");
  });

  it("counts only check runs from the GitHub Actions app", () => {
    const runs = allGreen().filter((r) => r.name !== "health");
    runs.push({ ...run("health"), app: "some-other-app" });
    const verdict = evaluate({ sha: SHA, checkRuns: runs, statuses: [] });
    expect(verdict.passed).toBe(false);
    const health = verdict.results.find((r) => r.context === "health");
    expect(health.state).toBe("missing");
    expect(health.ignoredCheckRuns[0].app).toBe("some-other-app");
  });

  it("never accepts a commit status for a required context", () => {
    const runs = allGreen().filter((r) => r.name !== "health");
    const verdict = evaluate({
      sha: SHA,
      checkRuns: runs,
      statuses: [{ context: "health", state: "success", creator: "github-actions[bot]" }],
    });
    expect(verdict.passed).toBe(false);
    expect(verdict.results.find((r) => r.context === "health").ignoredStatus).toEqual({
      state: "success",
      creator: "github-actions[bot]",
    });
  });

  it.each([
    [
      "pending",
      (runs) => Object.assign(runs[0], { status: "in_progress", conclusion: null }),
      "pending",
    ],
    ["queued", (runs) => Object.assign(runs[1], { status: "queued", conclusion: null }), "pending"],
    ["failed", (runs) => Object.assign(runs[2], { conclusion: "failure" }), "failed"],
    ["timed out", (runs) => Object.assign(runs[2], { conclusion: "timed_out" }), "failed"],
    ["cancelled", (runs) => Object.assign(runs[3], { conclusion: "cancelled" }), "cancelled"],
    ["missing", (runs) => runs.splice(4, 1), "missing"],
  ])("does not pass a %s context", (_label, mutate, state) => {
    const runs = allGreen();
    mutate(runs);
    const verdict = evaluate({ sha: SHA, checkRuns: runs, statuses: [] });
    expect(verdict.passed).toBe(false);
    expect(verdict.results.some((r) => r.state === state)).toBe(true);
    expect(verdict.blocking.join()).toContain(state);
  });

  it("uses the newest check run of a context (a re-run replaces the earlier one)", () => {
    const runs = allGreen();
    runs.push(run("health", "completed", "failure", 10_000));
    const failed = evaluate({ sha: SHA, checkRuns: runs, statuses: [] });
    expect(failed.passed).toBe(false);
    runs.push(run("health", "completed", "success", 10_001));
    const passed = evaluate({ sha: SHA, checkRuns: runs, statuses: [] });
    expect(passed.passed).toBe(true);
    expect(passed.results.find((r) => r.context === "health").history).toHaveLength(3);
  });

  it("can require the pull_request-only contexts too", () => {
    const verdict = evaluate({
      sha: SHA,
      checkRuns: allGreen(),
      statuses: [],
      requirePrOnly: true,
    });
    expect(verdict.passed).toBe(false);
    expect(verdict.blocking).toEqual(["pnpm audit (prod): missing", "Docs gate: missing"]);
  });

  it("classifies a failed pr-only context that does exist as failed", () => {
    const spec = REQUIRED_CONTEXTS.find((c) => c.context === "Docs gate");
    expect(classifyContext(spec, [run("Docs gate", "completed", "failure")], []).state).toBe(
      "failed",
    );
  });
});

describe("CLI", () => {
  const gh =
    (runs, statuses = [], { pathFor = (wf) => `.github/workflows/${wf}`, headSha = SHA } = {}) =>
    (args) => {
      if (args.includes("--paginate")) {
        return runs.map(({ workflowPath, ...raw }) => JSON.stringify(raw)).join("\n");
      }
      if (args[1].endsWith("/status")) return JSON.stringify({ statuses });
      const suite = /check_suite_id=(\d+)/.exec(args[1]);
      if (suite) {
        const workflow = WORKFLOWS[Number(suite[1]) - 1000];
        return JSON.stringify({ workflow_runs: [{ path: pathFor(workflow), head_sha: headSha }] });
      }
      throw new Error(`unexpected gh ${args.join(" ")}`);
    };
  const capture = () => {
    let text = "";
    return { out: { write: (chunk) => (text += chunk) }, text: () => text };
  };

  it("exits 0 / 1 / 2 for pass / not passed / read error", () => {
    const sink = capture();
    expect(main(["--sha", SHA], { gh: gh(allGreen()), out: sink.out })).toBe(0);
    expect(sink.text()).toContain("PASS");
    expect(main(["--sha", SHA, "--json"], { gh: gh([]), out: capture().out })).toBe(1);
    const failing = () => {
      throw new Error("HTTP 503");
    };
    expect(main(["--sha", SHA], { gh: failing, out: capture().out })).toBe(2);
  });

  it("resolves each check suite to its workflow file and requires the context's own", () => {
    const seen = [];
    const base = gh(allGreen());
    const counting = (args) => {
      seen.push(args.join(" "));
      return base(args);
    };
    expect(main(["--sha", SHA, "--repo", "o/r"], { gh: counting, out: capture().out })).toBe(0);
    const suiteCalls = seen.filter((c) => c.includes("check_suite_id="));
    expect(suiteCalls).toHaveLength(new Set(allGreen().map((r) => r.suite)).size);
    expect(suiteCalls[0]).toContain("repos/o/r/actions/runs?check_suite_id=");
  });

  it("rejects a same-named job from another workflow file", () => {
    const sink = capture();
    const code = main(["--sha", SHA, "--json"], {
      gh: gh(allGreen(), [], {
        pathFor: (wf) =>
          wf === "health.yml" ? ".github/workflows/impostor.yml" : `.github/workflows/${wf}`,
      }),
      out: sink.out,
    });
    expect(code).toBe(1);
    const health = JSON.parse(sink.text()).results.find((r) => r.context === "health");
    expect(health.state).toBe("missing");
    expect(health.ignoredCheckRuns[0].workflowPath).toBe(".github/workflows/impostor.yml");
  });

  it("rejects a check suite whose workflow run is for another commit", () => {
    expect(
      main(["--sha", SHA], {
        gh: gh(allGreen(), [], { headSha: "c".repeat(40) }),
        out: capture().out,
      }),
    ).toBe(1);
  });

  it("rejects a non-40-hex SHA and unknown flags", () => {
    expect(parseArgs(["--sha", "abc"])).toBeNull();
    expect(parseArgs(["--sha", SHA, "--bogus"])).toBeNull();
    expect(parseArgs(["--sha", SHA.toUpperCase()])).toBeNull();
  });

  it("queries exactly the given SHA", () => {
    const seen = [];
    main(["--sha", SHA, "--repo", "o/r"], {
      gh: (args) => {
        seen.push(args.join(" "));
        return args.includes("--paginate") ? "" : JSON.stringify({ statuses: [] });
      },
      out: capture().out,
    });
    expect(seen[0]).toContain(`repos/o/r/commits/${SHA}/check-runs`);
    expect(seen.at(-1)).toContain(`repos/o/r/commits/${SHA}/status`);
  });
});

describe("drift guards", () => {
  it("REQUIRED_CONTEXTS equals the main ruleset list in apply-main-ruleset.sh", () => {
    const script = readFileSync(join(scriptsDir, "apply-main-ruleset.sh"), "utf8");
    const ruleset = [...script.matchAll(/\{ "context": "([^"]+)" \}/g)].map((m) => m[1]);
    expect(ruleset.length).toBeGreaterThan(0);
    expect(REQUIRED_CONTEXTS.map((c) => c.context)).toEqual(ruleset);
  });

  it("classifies a context pr-only exactly when its workflow has no push-to-main trigger", () => {
    for (const spec of REQUIRED_CONTEXTS) {
      const workflow = parse(
        readFileSync(join(repoRoot, ".github/workflows", spec.workflow), "utf8"),
      );
      const on = workflow.on ?? workflow[true];
      const branches = on?.push?.branches;
      const pushesMain = Boolean(on?.push) && (!branches || branches.includes("main"));
      expect([spec.context, spec.prOnly]).toEqual([spec.context, !pushesMain]);
    }
  });

  it("names a job that exists in the stated workflow", () => {
    for (const spec of REQUIRED_CONTEXTS) {
      const workflow = parse(
        readFileSync(join(repoRoot, ".github/workflows", spec.workflow), "utf8"),
      );
      const names = Object.entries(workflow.jobs).map(([id, job]) =>
        String(job.name ?? id).replace("${{ matrix.platform.target }}", "*"),
      );
      const matches = names.some((name) =>
        name.includes("*")
          ? new RegExp(`^${name.replace(/[()]/g, "\\$&").replace("*", ".+")}$`).test(spec.context)
          : name === spec.context,
      );
      expect([spec.context, matches]).toEqual([spec.context, true]);
    }
  });
});
