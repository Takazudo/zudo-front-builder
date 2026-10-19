import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";

const workflow = parse(
  readFileSync(new URL("../../.github/workflows/exam.yml", import.meta.url), "utf8"),
);
const setup = parse(
  readFileSync(
    new URL("../../.github/actions/native-exam-setup/action.yml", import.meta.url),
    "utf8",
  ),
);

describe("native exam execution boundary", () => {
  it("executes the evidence suite through health's POSIX sh entrypoint", () => {
    const root = fileURLToPath(new URL("../../", import.meta.url));
    const wrapper = readFileSync(
      new URL("../../tests/unit/native-exam.sh", import.meta.url),
      "utf8",
    );
    expect(wrapper.split("\n")[0]).toBe("#!/bin/sh");
    expect(wrapper).not.toMatch(/\bpipefail\b/);
    const result = spawnSync("sh", ["tests/unit/native-exam.sh"], {
      cwd: root,
      encoding: "utf8",
    });
    expect(result.error).toBeUndefined();
    expect(result.status, result.stderr).toBe(0);
    expect(result.stderr).toMatch(/Ran [1-9]\d* tests?/);
    expect(result.stderr).toContain("OK");
  });
  it("keeps four isolated standard Mac jobs and a separate doctest job", () => {
    expect(workflow.jobs["macos-shards"].strategy).toEqual({
      "fail-fast": false,
      matrix: { shard: [1, 2, 3, 4] },
    });
    for (const name of ["macos-shards", "macos-doctests"]) {
      expect(workflow.jobs[name]["runs-on"]).toBe("macos-15");
      expect(workflow.jobs[name].outputs).toBeUndefined();
      expect(workflow.jobs[name].steps[0].with.ref).toBe("${{ needs.source.outputs.sha }}");
      expect(workflow.jobs[name].steps.at(-1).with["retention-days"]).toBe(3);
    }
  });
  it("does not cancel an existing exam and never closes an issue on manual qualification", () => {
    expect(workflow.concurrency["cancel-in-progress"]).toBe(false);
    expect(workflow.concurrency.group).toContain("exam-v2-");
    const close = workflow.jobs["close-tracking-issue-if-green"].steps.at(-1).if;
    expect(close).toContain("github.event_name == 'schedule'");
    expect(close).toContain("needs.source.outputs.sha == github.sha");
    expect(close).toContain("needs.quarantine-heavy.result == 'success'");
    expect(close).toContain("needs.quarantine-heavy.outputs.rc == '0'");
    expect(close).toContain("needs.macos-exam.result == 'success'");
    expect(close).toContain("needs.macos-exam.outputs.rc == '0'");
  });
  it("uses pinned native tools and installs before building", () => {
    const tool = (prefix) => setup.runs.steps.find((step) => step.uses?.startsWith(prefix)).with;
    expect(tool("dtolnay/").toolchain).toBe("1.99.1");
    expect(tool("actions/setup-node")["node-version"]).toBe("22.22.2");
    expect(tool("taiki-e/").tool).toBe("nextest@0.9.140");
    const preflight = setup.runs.steps.at(-1).run;
    expect(preflight.indexOf("pnpm install --frozen-lockfile")).toBeLessThan(
      preflight.indexOf("cargo build --locked"),
    );
    expect(preflight).toContain("test -x crates/zfb/binaries/esbuild/esbuild");
  });
  it("requires all native jobs and runs aggregate even after child failure", () => {
    const aggregate = workflow.jobs["macos-exam"];
    expect(aggregate.needs).toEqual(["source", "macos-shards", "macos-doctests"]);
    expect(aggregate.if).toBe("always()");
    const reconcile = aggregate.steps.find((step) => step.id === "aggregate");
    expect(reconcile.if).toBe("always()");
    expect(reconcile.run).toContain('exit "$rc"');
    expect(reconcile.env.SHARDS_RESULT).toBe("${{ needs.macos-shards.result }}");
  });
});
