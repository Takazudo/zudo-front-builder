import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { HELP, parseArgs } from "../cli.mjs";
import { parsePsTable, ownedProcesses } from "../lib/process-owner.mjs";
import { semanticErrors } from "../lib/profile.mjs";
import { SCHEMA_FILES, loadSchema, validate, validateAs } from "../lib/schema.mjs";
import { parseReadyUrl, devArgs } from "../lib/server.mjs";
import { distribution, nearestRank, summarize } from "../lib/stats.mjs";
import { HARNESS_DIR } from "../lib/util.mjs";

const example = (n) => JSON.parse(readFileSync(join(HARNESS_DIR, "profiles", n), "utf8"));

describe("schemas", () => {
  it("every schema file loads", () => {
    for (const name of Object.keys(SCHEMA_FILES)) expect(loadSchema(name).$id).toBeTruthy();
  });

  it("example profiles validate", () => {
    for (const n of [
      "tiny-internal-a0.example.json",
      "tiny-external-a0.example.json",
      "tiny-negative-controls.example.json",
    ]) {
      expect(validateAs("profile", example(n))).toEqual([]);
      expect(semanticErrors(example(n))).toEqual([]);
    }
  });

  it("rejects unknown keys, bad enums and a stale-predicate without expect", () => {
    const p = example("tiny-internal-a0.example.json");
    expect(validateAs("profile", { ...p, extra: 1 })).toContain("$: unexpected property extra");
    expect(validateAs("profile", { ...p, mode: "other" })[0]).toMatch(/not in/);
    expect(validateAs("profile", { ...p, productSha: "abc" })[0]).toMatch(/does not match/);
    const bad = { ...p, scenarios: [{ id: "x", ops: [{ op: "stale-predicate", article: "a" }] }] };
    expect(semanticErrors(bad)[0]).toMatch(/must declare expect/);
  });

  it("the fixture manifest validates against tiny-fixture.v1", () => {
    const m = JSON.parse(
      readFileSync(join(HARNESS_DIR, "fixtures/tiny-two-article/manifest.json"), "utf8"),
    );
    expect(validateAs("fixture", m)).toEqual([]);
  });

  it("validator handles null unions and integers", () => {
    expect(validate({ type: ["string", "null"] }, null)).toEqual([]);
    expect(validate({ type: "integer" }, 1.5)[0]).toMatch(/expected integer/);
  });
});

describe("stats", () => {
  it("nearest-rank p95 and median", () => {
    const v = Array.from({ length: 20 }, (_, i) => i + 1);
    expect(nearestRank(v, 95)).toBe(19);
    expect(distribution([3, 1, 2])).toMatchObject({
      n: 3,
      min: 1,
      median: 2,
      max: 3,
      p95NearestRank: 3,
    });
    expect(distribution([])).toMatchObject({ n: 0, median: null, p95NearestRank: null });
  });

  it("summaries are conditional on success", () => {
    const s = summarize(
      [
        {
          type: "attempt",
          outcome: "success",
          latencyMs: 10,
          op: "edit-title",
          mechanism: "native-full-reload",
        },
        { type: "attempt", outcome: "timeout", op: "edit-body" },
      ],
      { planned: 3 },
    );
    expect(s.latencyConditionalOnSuccessMs.n).toBe(1);
    expect(s.mechanisms).toEqual({ "native-full-reload": 1 });
  });
});

describe("cli and parsers", () => {
  it("parses flags", () => {
    expect(
      parseArgs(["run", "--profile", "p.json", "--label", "x", "--no-default-features"]),
    ).toEqual({ _: ["run"], profile: "p.json", label: "x", "no-default-features": true });
  });

  it("help names every command and the endpoint", () => {
    for (const c of [
      "record-build",
      "verify-identity",
      "probe-mac",
      "calibrate",
      "run",
      "schema",
      "pins",
    ])
      expect(HELP).toContain(c);
    expect(HELP).toContain("predicate confirmed + 2 rAF + binding delivery");
  });

  it("parses the zfb dev ready banner, ignoring ANSI and the bind line", () => {
    expect(parseReadyUrl("ℹ listening on 127.0.0.1:5173 (routes pending)\n")).toBeNull();
    expect(
      parseReadyUrl("\u001b[32m→\u001b[0m ready on \u001b[1mhttp://127.0.0.1:5173\u001b[0m\n"),
    ).toBe("http://127.0.0.1:5173");
    expect(devArgs({ port: { mode: "ephemeral" }, host: "127.0.0.1" })).toEqual([
      "dev",
      "--port",
      "0",
      "--host",
      "127.0.0.1",
    ]);
  });

  it("parses ps output and expands owned descendants", () => {
    const t = parsePsTable(
      "  10     1    10 Sat Nov 21 06:22:35 2026     /bin/zfb\n  11    10    10 Sat Nov 21 06:22:36 2026     node\n  12    11    12 Sat Nov 21 06:22:37 2026     child\n  13     1    13 Sat Nov 21 06:22:37 2026     other\n",
    );
    expect(t).toHaveLength(4);
    expect(t[0]).toMatchObject({
      pid: 10,
      ppid: 1,
      pgid: 10,
      lstart: "Sat Nov 21 06:22:35 2026",
      comm: "/bin/zfb",
    });
    expect(
      ownedProcesses(t, t[0])
        .map((p) => p.pid)
        .sort(),
    ).toEqual([10, 11, 12]);
  });
});
