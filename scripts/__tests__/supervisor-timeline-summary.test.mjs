// Tests for scripts/supervisor-timeline-summary.mjs.
//
// Pure-parse over fixture strings only — never spawns the docs dev
// supervisor. A sibling sub-task (#2904) runs that timing-sensitive suite
// concurrently in this same wave; contending for the exact startup budget
// under study would corrupt its measurement. The one filesystem read here
// (the drift-guard test) reads scripts/__tests__/docs-dev-supervisor.test.mjs
// as a text file — it never imports or executes it.

import { readFileSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import {
  DEFAULT_BUDGET_MS,
  DEFAULT_CASE,
  DEFAULT_THRESHOLD,
  EXIT_NO_SAMPLES,
  EXIT_OK,
  EXIT_STRICT,
  EXIT_USAGE,
  IDENTITY_FIELDS,
  distributionStats,
  evaluateRB,
  identityDrift,
  outcomeCounts,
  parseCliArgs,
  parseTimelineLine,
  parseTimelines,
  quantile,
  runCli,
  summarizeCase,
} from "../supervisor-timeline-summary.mjs";

// Real captured samples from the epic (#2902) / sub-issue (#2903) bodies.
const UP_BOOM_LINE =
  "[supervisor-timeline] case=up+boom outcome=ok total=540 runner=pnpm zudoDoc=5.15.0 runParallel=sha256:646f90cc300185cb fixtureShape=sha256:2d146d48587c00f5 env=sha256:ed62f5285936a0ca supervisor-spawned=1 first-stdout-byte=354 first-up-line=430 marker-file-created=437 first-stderr-byte=505 supervisor-error-line=505 supervisor-closed=540 sibling-death=540";
const UP_UP2_LINE =
  "[supervisor-timeline] case=up+up2 outcome=ok total=357 runner=pnpm zudoDoc=5.15.0 runParallel=sha256:646f90cc300185cb fixtureShape=sha256:2d146d48587c00f5 env=sha256:ed62f5285936a0ca supervisor-spawned=1 first-stdout-byte=233 first-up-line=292 marker-file-created=293 first-stderr-byte=343 supervisor-error-line=343 supervisor-closed=356 sibling-death=357";
const HIDDEN_LINE =
  "[supervisor-timeline] case=hidden outcome=expected-failure total=512 runner=pnpm zudoDoc=5.15.0 runParallel=sha256:646f90cc300185cb fixtureShape=sha256:2d146d48587c00f5 env=sha256:ed62f5285936a0ca supervisor-spawned=1 first-stdout-byte=184 hidden-pid-file=236";

const SAMPLE_LOG = [
  "stdout: some ordinary vitest noise",
  UP_BOOM_LINE,
  " > docs@0.0.0 dev",
  UP_UP2_LINE,
  HIDDEN_LINE,
  "PASS scripts/__tests__/docs-dev-supervisor.test.mjs",
].join("\n");

/** A second up+boom line, so distribution stats have n>1 to work with. */
function upBoomLine(overrides = {}) {
  const fields = {
    case: "up+boom",
    outcome: "ok",
    total: 600,
    runner: "pnpm",
    zudoDoc: "5.15.0",
    runParallel: "sha256:646f90cc300185cb",
    fixtureShape: "sha256:2d146d48587c00f5",
    env: "sha256:ed62f5285936a0ca",
    "first-stdout-byte": 380,
    "first-up-line": 470,
    ...overrides,
  };
  const { case: caseLabel, outcome, total, ...rest } = fields;
  const identity = IDENTITY_FIELDS.map((field) => `${field}=${rest[field]}`).join(" ");
  const marks = Object.entries(rest)
    .filter(([key]) => !IDENTITY_FIELDS.includes(key))
    .map(([key, value]) => `${key}=${value}`)
    .join(" ");
  return `[supervisor-timeline] case=${caseLabel} outcome=${outcome} total=${total} ${identity} ${marks}`;
}

/** Collects a fake stdout/stderr pair for asserting on runCli's output. */
function sink() {
  const out = [];
  const err = [];
  return {
    stdout: { write: (chunk) => out.push(chunk) },
    stderr: { write: (chunk) => err.push(chunk) },
    out: () => out.join(""),
    err: () => err.join(""),
  };
}

describe("quantile", () => {
  it("matches the locked formula exactly", () => {
    const sorted = [1, 2, 3, 4, 5];
    // floor(p * (n-1) + 0.5)
    expect(quantile(sorted, 0)).toBe(1);
    expect(quantile(sorted, 0.5)).toBe(3);
    expect(quantile(sorted, 0.9)).toBe(5);
    expect(quantile(sorted, 1)).toBe(5);
  });

  it("returns NaN for an empty distribution", () => {
    expect(quantile([], 0.5)).toBeNaN();
  });

  it("clamps the index so p=1 never overruns the array", () => {
    const sorted = [10, 20];
    expect(quantile(sorted, 0.99)).toBe(20);
  });
});

describe("distributionStats", () => {
  it("reports n/min/p50/p90/p99/max over unsorted input", () => {
    const stats = distributionStats([354, 292, 430, 184]);
    expect(stats.n).toBe(4);
    expect(stats.min).toBe(184);
    expect(stats.max).toBe(430);
    expect(stats.p50).toBe(quantile([184, 292, 354, 430], 0.5));
  });

  it("is all-NaN, n=0 for an empty distribution", () => {
    const stats = distributionStats([]);
    expect(stats.n).toBe(0);
    expect(stats.min).toBeNaN();
    expect(stats.max).toBeNaN();
  });
});

describe("parseTimelineLine", () => {
  it("parses a real captured up+boom line", () => {
    const record = parseTimelineLine(UP_BOOM_LINE);
    expect(record.case).toBe("up+boom");
    expect(record.outcome).toBe("ok");
    expect(record.total).toBe(540);
    expect(record.identity).toEqual({
      runner: "pnpm",
      zudoDoc: "5.15.0",
      runParallel: "sha256:646f90cc300185cb",
      fixtureShape: "sha256:2d146d48587c00f5",
      env: "sha256:ed62f5285936a0ca",
    });
    expect(record.marks["first-stdout-byte"]).toBe(354);
    expect(record.marks["first-up-line"]).toBe(430);
    expect(record.marks["supervisor-closed"]).toBe(540);
  });

  it("parses the hidden case, which never reaches first-up-line", () => {
    const record = parseTimelineLine(HIDDEN_LINE);
    expect(record.case).toBe("hidden");
    expect(record.outcome).toBe("expected-failure");
    expect(record.marks["first-up-line"]).toBeUndefined();
    expect(record.marks["hidden-pid-file"]).toBe(236);
  });

  it("ignores an ordinary line with no [supervisor-timeline] tag", () => {
    expect(parseTimelineLine("PASS some.test.mjs")).toBeNull();
    expect(parseTimelineLine("")).toBeNull();
  });

  it("throws on a tagged line missing a required field", () => {
    expect(() => parseTimelineLine("[supervisor-timeline] outcome=ok total=1 runner=pnpm")).toThrow(
      /missing "case"/,
    );
  });

  it("throws on a tagged line with an unknown outcome value", () => {
    expect(() =>
      parseTimelineLine(
        "[supervisor-timeline] case=x outcome=bogus total=1 runner=pnpm zudoDoc=1 runParallel=sha256:a fixtureShape=sha256:b env=sha256:c",
      ),
    ).toThrow(/unknown outcome "bogus"/);
  });

  it("throws on a bad key=value token", () => {
    expect(() => parseTimelineLine("[supervisor-timeline] case=x notakeyvalue")).toThrow(
      /bad key=value token/,
    );
  });

  it("throws on a non-integer ms value", () => {
    expect(() =>
      parseTimelineLine(
        "[supervisor-timeline] case=x outcome=ok total=abc runner=pnpm zudoDoc=1 runParallel=sha256:a fixtureShape=sha256:b env=sha256:c",
      ),
    ).toThrow(/non-integer ms/);
  });
});

describe("parseTimelines", () => {
  it("parses only the tagged lines out of a full vitest-log-shaped blob, in order", () => {
    const records = parseTimelines(SAMPLE_LOG);
    expect(records.map((record) => record.case)).toEqual(["up+boom", "up+up2", "hidden"]);
  });

  it("returns an empty array for input with no tagged lines", () => {
    expect(parseTimelines("just some ordinary log\nPASS foo.test.mjs\n")).toEqual([]);
  });

  it("throws when any tagged line is malformed, even amid valid ones", () => {
    const blob = [UP_BOOM_LINE, "[supervisor-timeline] case=broken"].join("\n");
    expect(() => parseTimelines(blob)).toThrow(/malformed/);
  });
});

describe("outcomeCounts", () => {
  it("tallies all three outcome values", () => {
    const records = parseTimelines(SAMPLE_LOG);
    expect(outcomeCounts(records)).toEqual({ ok: 2, "expected-failure": 1, failed: 0 });
  });
});

describe("summarizeCase", () => {
  it("computes pre-UP, package-manager-startup, server-listen, and whole-case distributions", () => {
    const records = [
      UP_BOOM_LINE,
      upBoomLine({ total: 600, "first-stdout-byte": 380, "first-up-line": 470 }),
    ].map((line) => parseTimelineLine(line));
    const summary = summarizeCase(records);
    expect(summary.preUp.n).toBe(2);
    expect(summary.preUp.max).toBe(470);
    expect(summary.pkgStartup.max).toBe(380);
    // server-listen is the per-record difference, not a difference of aggregates.
    expect(summary.serverListen.min).toBe(430 - 354);
    expect(summary.serverListen.max).toBe(470 - 380);
    expect(summary.wholeCase.n).toBe(2);
    expect(summary.wholeCase.max).toBe(600);
  });

  it("leaves pre-UP/server-listen empty (n=0) for a case that never reaches first-up-line", () => {
    // hidden has first-stdout-byte (package-manager startup happened) but no
    // first-up-line -- the pre-UP timeout is the point of that case (#2887).
    const records = [parseTimelineLine(HIDDEN_LINE)];
    const summary = summarizeCase(records);
    expect(summary.preUp.n).toBe(0);
    expect(summary.pkgStartup.n).toBe(1);
    expect(summary.serverListen.n).toBe(0);
    // total is a required field, so whole-case is still populated.
    expect(summary.wholeCase.n).toBe(1);
  });
});

describe("identityDrift", () => {
  it("reports no drift when every record shares identical identity fields", () => {
    const records = parseTimelines(SAMPLE_LOG).filter((record) => record.case === "up+boom");
    const drift = identityDrift(records);
    expect(drift.hasDrift).toBe(false);
    expect(drift.driftedFields).toEqual([]);
  });

  it.each(IDENTITY_FIELDS)("detects drift when %s differs across records", (field) => {
    const base = parseTimelineLine(UP_BOOM_LINE);
    const mutated = parseTimelineLine(upBoomLine({ [field]: `${base.identity[field]}-mutated` }));
    const drift = identityDrift([base, mutated]);
    expect(drift.hasDrift).toBe(true);
    expect(drift.driftedFields).toEqual([field]);
    expect(drift.distinct[field]).toHaveLength(2);
  });
});

describe("evaluateRB", () => {
  it("does not trip when max pre-UP is comfortably under threshold x budget", () => {
    const rb = evaluateRB({ max: 430 }, 10_000, 0.75);
    expect(rb.boundary).toBe(7_500);
    expect(rb.tripped).toBe(false);
  });

  it("trips exactly at the boundary (>=), not only strictly above it", () => {
    const rb = evaluateRB({ max: 7_500 }, 10_000, 0.75);
    expect(rb.tripped).toBe(true);
  });

  it("does not trip when there is no pre-UP data at all", () => {
    const rb = evaluateRB({ max: NaN }, 10_000, 0.75);
    expect(rb.tripped).toBe(false);
  });
});

describe("parseCliArgs", () => {
  it("applies documented defaults with no flags", () => {
    const options = parseCliArgs([]);
    expect(options.caseLabel).toBe(DEFAULT_CASE);
    expect(options.budgetMs).toBe(DEFAULT_BUDGET_MS);
    expect(options.threshold).toBe(DEFAULT_THRESHOLD);
    expect(options.strict).toBe(false);
    expect(options.files).toEqual([]);
  });

  it("parses all flags plus positional files", () => {
    const options = parseCliArgs([
      "--case",
      "up+up2",
      "--budget-ms",
      "5000",
      "--threshold",
      "0.5",
      "--strict",
      "a.log",
      "b.log",
    ]);
    expect(options).toEqual({
      caseLabel: "up+up2",
      budgetMs: 5000,
      threshold: 0.5,
      strict: true,
      files: ["a.log", "b.log"],
    });
  });

  it("throws on an unknown flag", () => {
    expect(() => parseCliArgs(["--nope"])).toThrow(/unknown flag: --nope/);
  });

  it("throws when --case is missing its value", () => {
    expect(() => parseCliArgs(["--case"])).toThrow(/--case requires a value/);
  });

  it("throws when --budget-ms is not a positive number", () => {
    expect(() => parseCliArgs(["--budget-ms", "abc"])).toThrow(
      /--budget-ms requires a positive number/,
    );
    expect(() => parseCliArgs(["--budget-ms", "-1"])).toThrow(
      /--budget-ms requires a positive number/,
    );
    expect(() => parseCliArgs(["--budget-ms", "0"])).toThrow(
      /--budget-ms requires a positive number/,
    );
  });

  it("throws when --threshold is not a positive number", () => {
    expect(() => parseCliArgs(["--threshold", "nope"])).toThrow(
      /--threshold requires a positive number/,
    );
  });
});

describe("runCli exit-code precedence (locked in #2902/#2903)", () => {
  it("step 1: usage/malformed -> 64, an unknown flag", async () => {
    const s = sink();
    const code = await runCli(["--nope"], s);
    expect(code).toBe(EXIT_USAGE);
    expect(s.err()).toMatch(/usage error/);
  });

  it("step 1: usage/malformed -> 64, a missing flag value", async () => {
    const s = sink();
    expect(await runCli(["--budget-ms"], s)).toBe(EXIT_USAGE);
  });

  it("step 1: usage/malformed -> 64, an unreadable file", async () => {
    const s = sink();
    const code = await runCli(["/nonexistent/path/does-not-exist.log"], s);
    expect(code).toBe(EXIT_USAGE);
    expect(s.err()).toMatch(/cannot read file/);
  });

  it("step 1: usage/malformed -> 64, a tagged line that fails to parse -- never conflated with no-data (1)", async () => {
    const s = sink();
    const code = await runCli([], { ...s, stdin: "[supervisor-timeline] case=broken\n" });
    expect(code).toBe(EXIT_USAGE);
    expect(code).not.toBe(EXIT_NO_SAMPLES);
  });

  it("step 2: --strict and outcome=failed anywhere -> 2, even when --case filters that case out of the report", async () => {
    const failedElsewhere = upBoomLine({ case: "hidden", outcome: "failed" }); // real failure, not expected-failure
    const s = sink();
    const code = await runCli(["--strict", "--case", "up+boom"], {
      ...s,
      stdin: [UP_BOOM_LINE, failedElsewhere].join("\n"),
    });
    expect(code).toBe(EXIT_STRICT);
  });

  it("step 2 precedes step 3: --strict and a failed line elsewhere wins over an empty selected case", async () => {
    const failedElsewhere = upBoomLine({ case: "other", outcome: "failed" });
    const s = sink();
    const code = await runCli(["--strict", "--case", "does-not-exist"], {
      ...s,
      stdin: failedElsewhere,
    });
    expect(code).toBe(EXIT_STRICT);
  });

  it("does NOT trip on outcome=expected-failure under --strict", async () => {
    const s = sink();
    const code = await runCli(["--strict", "--case", "hidden"], { ...s, stdin: HIDDEN_LINE });
    expect(code).toBe(EXIT_OK);
  });

  it("step 3: no samples for the selected --case -> 1", async () => {
    const s = sink();
    const code = await runCli(["--case", "does-not-exist"], { ...s, stdin: UP_BOOM_LINE });
    expect(code).toBe(EXIT_NO_SAMPLES);
  });

  it("step 3: totally empty input -> 1, loudly", async () => {
    const s = sink();
    const code = await runCli([], { ...s, stdin: "no timeline lines in this log at all\n" });
    expect(code).toBe(EXIT_NO_SAMPLES);
    expect(s.err()).toMatch(/NO \[supervisor-timeline\] LINES FOUND/);
  });

  it("step 4: --strict and INPUT drift -> 2", async () => {
    const drifted = upBoomLine({ runner: "npm" });
    const s = sink();
    const code = await runCli(["--strict"], { ...s, stdin: [UP_BOOM_LINE, drifted].join("\n") });
    expect(code).toBe(EXIT_STRICT);
  });

  it("step 4: --strict and an R-B trip -> 2", async () => {
    const s = sink();
    const code = await runCli(["--strict", "--budget-ms", "500", "--threshold", "0.5"], {
      ...s,
      stdin: UP_BOOM_LINE, // first-up-line=430 >= 0.5 * 500 = 250
    });
    expect(code).toBe(EXIT_STRICT);
  });

  it("step 4 does not fire without --strict, even with drift and an R-B trip present", async () => {
    const drifted = upBoomLine({ runner: "npm" });
    const s = sink();
    const code = await runCli(["--budget-ms", "500", "--threshold", "0.5"], {
      ...s,
      stdin: [UP_BOOM_LINE, drifted].join("\n"),
    });
    expect(code).toBe(EXIT_OK);
  });

  it("step 5: otherwise -> 0, and prints the report", async () => {
    const s = sink();
    const code = await runCli([], { ...s, stdin: SAMPLE_LOG });
    expect(code).toBe(EXIT_OK);
    expect(s.out()).toMatch(/case "up\+boom"/);
    expect(s.out()).toMatch(/pre-UP/);
    expect(s.out()).toMatch(/R-B verdict/);
  });
});

describe("runCli input sources", () => {
  it("reads from files given as positional args, concatenating them", async () => {
    const dir = mkdtempSync(join(tmpdir(), "supervisor-timeline-summary-test-"));
    try {
      const fileA = join(dir, "a.log");
      const fileB = join(dir, "b.log");
      writeFileSync(fileA, `${UP_BOOM_LINE}\n`);
      writeFileSync(fileB, `${UP_UP2_LINE}\n`);
      const s = sink();
      const code = await runCli([fileA, fileB], s);
      expect(code).toBe(EXIT_OK);
      expect(s.out()).toMatch(/parsed 2 \[supervisor-timeline\] line\(s\)/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("reads from stdin when no files are given", async () => {
    const s = sink();
    const code = await runCli([], { ...s, stdin: UP_BOOM_LINE });
    expect(code).toBe(EXIT_OK);
  });
});

describe("drift guard: --budget-ms default tracks PROCESS_TIMEOUT_MS", () => {
  it("matches the constant docs-dev-supervisor.test.mjs measures pre-UP against", () => {
    const source = readFileSync(new URL("./docs-dev-supervisor.test.mjs", import.meta.url), "utf8");
    const match = source.match(/const PROCESS_TIMEOUT_MS\s*=\s*([\d_]+)\s*;/);
    expect(
      match,
      "PROCESS_TIMEOUT_MS declaration not found in docs-dev-supervisor.test.mjs",
    ).not.toBeNull();
    const processTimeoutMs = Number(match[1].replace(/_/g, ""));
    expect(DEFAULT_BUDGET_MS).toBe(processTimeoutMs);
  });
});
