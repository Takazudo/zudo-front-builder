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

import { TIMELINE_SAMPLES } from "./fixtures/load-timeline-samples.mjs";
import {
  ENV_IDENTITY_VERSION,
  STEERING_ENV_KEYS,
  steeringEnvIdentity,
} from "../supervisor-env-identity.mjs";
import {
  DEFAULT_BUDGET_MS,
  DEFAULT_CASE,
  DEFAULT_THRESHOLD,
  EXIT_NO_SAMPLES,
  EXIT_OK,
  EXIT_STRICT,
  EXIT_USAGE,
  IDENTITY_FIELDS,
  analyzeCase,
  distributionStats,
  envVersionCohorts,
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

// Real captured samples from the epic (#2902) / sub-issue (#2903) bodies,
// canonical-sourced from the corpus shared with the other supervisor test
// harnesses (#2930) -- see scripts/__tests__/fixtures/supervisor-timeline-samples.txt.
const UP_BOOM_LINE = TIMELINE_SAMPLES.UP_BOOM_LINE;
const UP_UP2_LINE = TIMELINE_SAMPLES.UP_UP2_LINE;
const HIDDEN_LINE = TIMELINE_SAMPLES.HIDDEN_LINE;
// The same up+boom record in its pre-#2933 wire form (bare `env=sha256:…`):
// an already-emitted record, which must parse as implicit v1.
const UNVERSIONED_UP_BOOM_LINE = TIMELINE_SAMPLES.UNVERSIONED_UP_BOOM_LINE;

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
    env: "v1:sha256:ed62f5285936a0ca",
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
  // Regression for the residual half of the /code-review high-severity finding:
  // the emitting test file contains the tag inside a string literal, so vitest
  // prints that code frame whenever a supervisor test fails. The frame's first
  // token after the tag (`case=hidden");`) is key=value-shaped, so the
  // first-token guard alone lets it through and it then throws on the missing
  // `outcome` -- aborting the whole run with exit 64 exactly in the R-A case.
  it("ignores a vitest code frame whose first token is key=value-shaped", () => {
    const frame =
      '  913|       expect(timelineLines[0]).toContain("[supervisor-timeline] case=hidden");';
    expect(parseTimelineLine(frame)).toBeNull();
  });

  it("ignores a single-quoted or backticked source mention of the tag", () => {
    expect(
      parseTimelineLine("const t = '[supervisor-timeline] case=up+boom outcome=ok';"),
    ).toBeNull();
    expect(
      parseTimelineLine("const t = `[supervisor-timeline] case=up+boom outcome=ok`;"),
    ).toBeNull();
  });

  it("still parses a real emission behind the pnpm -r reporter prefix", () => {
    const real =
      ". test: [supervisor-timeline] case=up+boom outcome=ok total=540 runner=pnpm " +
      "zudoDoc=5.15.0 runParallel=a fixtureShape=b env=c first-stdout-byte=354 first-up-line=430";
    expect(parseTimelineLine(real)).toMatchObject({ case: "up+boom", outcome: "ok", total: 540 });
  });
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
      env: "v1:sha256:ed62f5285936a0ca",
    });
    expect(record.envVersion).toBe(1);
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

  // Regression for the join defect #2905 caught live: `pnpm -r`'s parallel
  // reporter prefixes every line from a root-level test file with a package
  // label (". test: ") before the [supervisor-timeline] tag. The documented
  // pipeline (`pnpm test:workspace` piped through `grep -h`) hands the
  // summarizer exactly this shape, not the bare tagged line the other
  // fixtures above use.
  it("parses a line still carrying its pnpm -r package-label prefix", () => {
    const record = parseTimelineLine(`. test: ${UP_BOOM_LINE}`);
    expect(record.case).toBe("up+boom");
    expect(record.outcome).toBe("ok");
    expect(record.total).toBe(540);
  });

  // The tag is deliberately unanchored (pnpm -r prefixes it), so lines that
  // merely QUOTE it must be ignored rather than throw: a vitest code frame of
  // the emitting test file, and this tool's own banner/report lines when a
  // whole CI job log is grepped. Throwing there aborts the entire analysis
  // with exit 64 and discards the real samples beside it.
  it("ignores a line that only quotes the tag inside source code", () => {
    expect(parseTimelineLine('        if (text.startsWith("[supervisor-timeline]")) {')).toBeNull();
  });

  it("ignores this tool's own banner and report lines", () => {
    expect(
      parseTimelineLine("No input matched the [supervisor-timeline] tag -- nothing was parsed."),
    ).toBeNull();
    expect(parseTimelineLine("parsed 2 [supervisor-timeline] line(s)")).toBeNull();
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

  it("keeps the real samples when a quoted-tag line sits beside them", () => {
    const blob = [
      "FAIL scripts/__tests__/docs-dev-supervisor.test.mjs",
      '  913|         if (text.startsWith("[supervisor-timeline]")) {',
      UP_BOOM_LINE,
    ].join("\n");
    expect(parseTimelines(blob).map((record) => record.case)).toEqual(["up+boom"]);
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
    expect(options.allowDrift).toEqual([]);
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
      "--allow-drift",
      "env,runner",
      "a.log",
      "b.log",
    ]);
    expect(options).toEqual({
      caseLabel: "up+up2",
      budgetMs: 5000,
      threshold: 0.5,
      strict: true,
      allowDrift: ["env", "runner"],
      files: ["a.log", "b.log"],
    });
  });

  it("throws when --allow-drift is missing its value", () => {
    expect(() => parseCliArgs(["--allow-drift"])).toThrow(/--allow-drift requires a value/);
  });

  it("throws on an unknown --allow-drift field", () => {
    expect(() => parseCliArgs(["--allow-drift", "bogus"])).toThrow(
      /unknown field for --allow-drift: "bogus"/,
    );
  });

  it("throws when --allow-drift is given an empty or comma-only list", () => {
    expect(() => parseCliArgs(["--allow-drift", ""])).toThrow(
      /--allow-drift requires at least one field name/,
    );
    expect(() => parseCliArgs(["--allow-drift", ","])).toThrow(
      /--allow-drift requires at least one field name/,
    );
  });

  it("accumulates a repeated --allow-drift instead of replacing the earlier list", () => {
    const options = parseCliArgs(["--allow-drift", "env", "--allow-drift", "runner,env"]);
    expect(options.allowDrift).toEqual(["env", "runner"]);
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

  it("step 4: --allow-drift suppresses --strict for the listed field, but the drift is still printed", async () => {
    const drifted = upBoomLine({ env: "sha256:mutated-env" });
    const s = sink();
    const code = await runCli(["--strict", "--allow-drift", "env"], {
      ...s,
      stdin: [UP_BOOM_LINE, drifted].join("\n"),
    });
    expect(code).toBe(EXIT_OK);
    expect(s.out()).toMatch(/INPUT DRIFT DETECTED/);
    expect(s.out()).toMatch(
      /env: v1:sha256:ed62f5285936a0ca, v1:sha256:mutated-env \(allowed by --allow-drift\)/,
    );
    expect(s.out()).toMatch(/Every drifted field is allow-listed/);
  });

  it("step 4: --allow-drift on one field does not cover drift on another -> 2", async () => {
    const drifted = upBoomLine({ env: "sha256:mutated-env", runner: "npm" });
    const s = sink();
    const code = await runCli(["--strict", "--allow-drift", "env"], {
      ...s,
      stdin: [UP_BOOM_LINE, drifted].join("\n"),
    });
    expect(code).toBe(EXIT_STRICT);
    expect(s.out()).toMatch(/env: .* \(allowed by --allow-drift\)/);
    expect(s.out()).toMatch(/runner: pnpm, npm\n/);
    expect(s.out()).not.toMatch(/Every drifted field is allow-listed/);
  });

  it("step 1: usage/malformed -> 64, an unknown --allow-drift field", async () => {
    const s = sink();
    const code = await runCli(["--allow-drift", "bogus"], { ...s, stdin: UP_BOOM_LINE });
    expect(code).toBe(EXIT_USAGE);
    expect(s.err()).toMatch(/unknown field for --allow-drift: "bogus"/);
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

// #2930's emitter-compatibility assertion: the shared corpus
// (scripts/__tests__/fixtures/supervisor-timeline-samples.txt) is
// hand-authored, not captured from a real run, so nothing forces it to keep
// matching the real emitter (docs-dev-supervisor.test.mjs's timelineLine())
// as that emitter changes. This describe block reads the emitter's own
// source -- the same technique the drift guard above already uses -- and
// fails loudly if either the identity-field order or the record's overall
// token order has moved out from under the corpus.
describe("emitter compatibility: the corpus still matches docs-dev-supervisor.test.mjs's timelineLine()", () => {
  const source = readFileSync(new URL("./docs-dev-supervisor.test.mjs", import.meta.url), "utf8");

  // The identity array literal inside timelineLine():
  //   const identity = [`runner=${...}`, `zudoDoc=${...}`, ...].join(" ");
  // Extracting the field NAMES from the emitter's own source (rather than
  // hardcoding them here) is what makes both assertions below fail loudly on
  // a real rename, instead of only on a rename nobody remembered to mirror.
  // Not run inside an `it` (no test-scoped assertions here): every test in
  // this block needs the extracted names, and a match failure means the
  // extraction itself is broken, not any one sample -- collection-time
  // failure of the whole file is the right blast radius for that.
  const identityBlock = source.match(/const identity = \[([\s\S]*?)\]\.join\(" "\);/);
  if (identityBlock === null) {
    throw new Error(
      "identity array literal not found in docs-dev-supervisor.test.mjs's timelineLine()",
    );
  }
  const identityFieldNames = [...identityBlock[1].matchAll(/`([\w-]+)=\$\{/g)].map((m) => m[1]);
  if (identityFieldNames.length === 0) {
    throw new Error("no identity field names extracted from timelineLine()'s identity array");
  }

  it("the identity field order timelineLine() emits matches IDENTITY_FIELDS", () => {
    // This is the tie that makes the corpus assertion below meaningful: the
    // summarizer's own IDENTITY_FIELDS (used to parse every sample) must
    // agree with what the emitter actually produces, not just with itself.
    expect(identityFieldNames).toEqual(IDENTITY_FIELDS);
  });

  it("the record's case/outcome/total/identity/marks stay in that relative order", () => {
    const returnStatement = source.match(/return `\[supervisor-timeline\][\s\S]*?`;/);
    expect(returnStatement, "timelineLine()'s return template literal not found").not.toBeNull();
    const snippet = returnStatement[0];
    const indices = {
      case: snippet.indexOf("case=$"),
      outcome: snippet.indexOf("outcome=$"),
      total: snippet.indexOf("total=$"),
      identity: snippet.indexOf("${identity}"),
      marks: snippet.indexOf("marks"),
    };
    for (const [field, index] of Object.entries(indices)) {
      expect(
        index,
        `"${field}" token not found in timelineLine()'s return template`,
      ).toBeGreaterThan(-1);
    }
    expect(indices.case).toBeLessThan(indices.outcome);
    expect(indices.outcome).toBeLessThan(indices.total);
    expect(indices.total).toBeLessThan(indices.identity);
    expect(indices.identity).toBeLessThan(indices.marks);
  });

  it("every corpus sample matches the emitter's produced shape", () => {
    const identityPattern = identityFieldNames.map((field) => `${field}=\\S+`).join(" ");
    const recordShape = new RegExp(
      `^\\[supervisor-timeline\\] case=\\S+ outcome=\\S+ total=\\d+ ${identityPattern}( \\S+=\\S+)+$`,
    );

    const sampleNames = Object.keys(TIMELINE_SAMPLES);
    expect(sampleNames.length).toBeGreaterThan(0);
    for (const name of sampleNames) {
      expect(
        recordShape.test(TIMELINE_SAMPLES[name]),
        `${name} does not match: ${TIMELINE_SAMPLES[name]}`,
      ).toBe(true);
    }
  });

  // The `v<N>:` prefix is produced inside steeringEnvIdentity(), not in the
  // identity array literal the assertions above read, so it needs its own
  // tie to the live contract: a corpus sample must carry the version the
  // emitter stamps TODAY, and only a sample declared UNVERSIONED_* may carry
  // the pre-#2933 bare form (the already-emitted shape the parser must keep
  // accepting). Bumping ENV_IDENTITY_VERSION without touching the corpus
  // therefore fails here, by design.
  it("every corpus sample carries the live env contract version, except the declared UNVERSIONED_* legacy samples", () => {
    const versioned = new RegExp(`^v${ENV_IDENTITY_VERSION}:sha256:[0-9a-f]{16}$`);
    const legacy = /^sha256:[0-9a-f]{16}$/;
    const names = Object.keys(TIMELINE_SAMPLES);
    expect(names.some((name) => name.startsWith("UNVERSIONED_"))).toBe(true);
    for (const name of names) {
      const env = / env=(\S+)/.exec(TIMELINE_SAMPLES[name])?.[1];
      expect(env, `${name} has no env= token`).toBeDefined();
      expect(
        (name.startsWith("UNVERSIONED_") ? legacy : versioned).test(env),
        `${name} carries env=${env}`,
      ).toBe(true);
    }
  });
});

// #2933's cohort semantics -- the seven bullets in the summarizer's doc
// comment, one describe block each so a regression names the bullet it
// broke. Records are built with upBoomLine() so only the token under test
// varies; the two "real contract" cases at the end use the identity module
// itself to stage a genuine STEERING_ENV_KEYS bump.
describe("env identity contract cohorts (#2933)", () => {
  const V2 = "v2:sha256:0000000000000002";

  describe("1. an unversioned env= record is v1", () => {
    it("parses the legacy wire form to the same identity as the explicit v1 sample", () => {
      const legacy = parseTimelineLine(UNVERSIONED_UP_BOOM_LINE);
      const fresh = parseTimelineLine(UP_BOOM_LINE);
      expect(legacy.envVersion).toBe(1);
      expect(legacy.identity.env).toBe("v1:sha256:ed62f5285936a0ca");
      expect(legacy.identity).toEqual(fresh.identity);
      expect(legacy.raw).toBe(UNVERSIONED_UP_BOOM_LINE);
    });

    it("reports no drift and no split across a population straddling the prefix", async () => {
      expect(
        identityDrift([UNVERSIONED_UP_BOOM_LINE, UP_BOOM_LINE].map(parseTimelineLine)).hasDrift,
      ).toBe(false);
      const s = sink();
      const code = await runCli(["--strict"], {
        ...s,
        stdin: [UNVERSIONED_UP_BOOM_LINE, UP_BOOM_LINE].join("\n"),
      });
      expect(code).toBe(EXIT_OK);
      expect(s.out()).not.toMatch(/DRIFT|SPLIT/);
      expect(s.out()).toMatch(/identity env: v1:sha256:ed62f5285936a0ca\n/);
    });
  });

  describe("2. R-A stays global across versions", () => {
    it("--strict trips on outcome=failed under any contract version, even outside --case", async () => {
      const s = sink();
      const code = await runCli(["--strict", "--case", "up+boom"], {
        ...s,
        stdin: [UP_BOOM_LINE, upBoomLine({ case: "hidden", outcome: "failed", env: V2 })].join(
          "\n",
        ),
      });
      expect(code).toBe(EXIT_STRICT);
      expect(s.err()).toMatch(/outcome=failed present/);
    });
  });

  describe("3. identity drift is evaluated within each cohort", () => {
    it("a field that differs only between cohorts is not drift", async () => {
      const s = sink();
      const code = await runCli(["--strict"], {
        ...s,
        stdin: [UP_BOOM_LINE, upBoomLine({ env: V2, runner: "npm" })].join("\n"),
      });
      expect(code).toBe(EXIT_OK);
      expect(s.out()).not.toMatch(/INPUT DRIFT DETECTED/);
      // Still visible, per cohort.
      expect(s.out()).toMatch(/identity runner: pnpm\n/);
      expect(s.out()).toMatch(/identity runner: npm\n/);
    });

    it("the same difference inside one cohort is drift, and --strict trips on it", async () => {
      const s = sink();
      const code = await runCli(["--strict"], {
        ...s,
        stdin: [UP_BOOM_LINE, upBoomLine({ env: V2 }), upBoomLine({ env: V2, runner: "npm" })].join(
          "\n",
        ),
      });
      expect(code).toBe(EXIT_STRICT);
      expect(s.out()).toMatch(/runner: pnpm, npm\n/);
    });
  });

  describe("4. R-B and the timing distributions are computed per cohort", () => {
    const records = [
      UP_BOOM_LINE, // v1, first-up-line=430
      upBoomLine({ env: V2, "first-up-line": 7600, total: 7900 }),
      upBoomLine({ env: V2, "first-up-line": 7700, total: 8000 }),
    ].map(parseTimelineLine);

    it("groups a case by version, ascending", () => {
      const cohorts = envVersionCohorts(records);
      expect(cohorts.map((cohort) => cohort.version)).toEqual([1, 2]);
      expect(cohorts.map((cohort) => cohort.records.length)).toEqual([1, 2]);
    });

    it("never pools the distributions or the verdict across versions", async () => {
      const { cohorts, split } = analyzeCase(records, {
        budgetMs: DEFAULT_BUDGET_MS,
        threshold: DEFAULT_THRESHOLD,
      });
      expect(split).toBe(true);
      expect(cohorts[0].summary.preUp).toMatchObject({ n: 1, max: 430 });
      expect(cohorts[1].summary.preUp).toMatchObject({ n: 2, min: 7600, max: 7700 });
      expect(cohorts[0].rb.tripped).toBe(false);
      expect(cohorts[1].rb.tripped).toBe(true);

      const s = sink();
      await runCli([], { ...s, stdin: records.map((record) => record.raw).join("\n") });
      expect(s.out()).toMatch(/pre-UP \(spawn -> UP line\): n=1 min=430ms .* max=430ms\n/);
      expect(s.out()).toMatch(/pre-UP \(spawn -> UP line\): n=2 min=7600ms .* max=7700ms\n/);
      expect(s.out()).toMatch(/R-B verdict \[env contract v1\]: max pre-UP=430ms .* -> ok\n/);
      expect(s.out()).toMatch(/R-B verdict \[env contract v2\]: max pre-UP=7700ms .* -> TRIPPED\n/);
      // No pooled distribution anywhere: none of the four distribution lines
      // may carry the combined n=3. Deliberately NOT a bare /n=3/ -- the
      // required split report ("case ... (n=3) spans 2 env contract
      // versions") states that total on purpose, and a bare match would
      // forbid the very line semantic 7 asks for.
      expect(s.out()).not.toMatch(
        /(pre-UP \(spawn -> UP line\)|of which package-manager startup|of which server listen after that|whole case \(total\)): n=3/,
      );
    });
  });

  describe("5. --strict fails when ANY cohort trips R-B or carries disallowed drift", () => {
    it("an R-B trip confined to one cohort is still a strict finding", async () => {
      const s = sink();
      const code = await runCli(["--strict"], {
        ...s,
        stdin: [UP_BOOM_LINE, upBoomLine({ env: V2, "first-up-line": 7600, total: 7900 })].join(
          "\n",
        ),
      });
      expect(code).toBe(EXIT_STRICT);
    });

    it("drift confined to the OLD cohort is still a strict finding", async () => {
      const s = sink();
      const code = await runCli(["--strict"], {
        ...s,
        stdin: [UP_BOOM_LINE, upBoomLine({ runner: "npm" }), upBoomLine({ env: V2 })].join("\n"),
      });
      expect(code).toBe(EXIT_STRICT);
    });
  });

  describe("6. a version split is reported prominently but is not a strict finding", () => {
    it("two clean cohorts under bare --strict exit 0 with the split banner", async () => {
      const s = sink();
      const code = await runCli(["--strict"], {
        ...s,
        stdin: [UP_BOOM_LINE, upBoomLine({ env: V2 }), upBoomLine({ env: V2 })].join("\n"),
      });
      expect(code).toBe(EXIT_OK);
      expect(s.out()).toMatch(
        /^case "up\+boom" \(n=3\) spans 2 env contract versions: v1 \(n=1\), v2 \(n=2\)$/m,
      );
      expect(s.out()).toMatch(/!!! ENV IDENTITY CONTRACT VERSION SPLIT !!!/);
      expect(s.out()).toMatch(/the split itself is not a --strict finding/);
      expect(s.out()).toMatch(/^case "up\+boom" \(n=1\) \[env contract v1\]:$/m);
      expect(s.out()).toMatch(/^case "up\+boom" \(n=2\) \[env contract v2\]:$/m);
    });

    it("a single cohort prints no split banner and names its version", async () => {
      const s = sink();
      const code = await runCli(["--strict"], { ...s, stdin: SAMPLE_LOG });
      expect(code).toBe(EXIT_OK);
      expect(s.out()).not.toMatch(/SPLIT|spans/);
      expect(s.out()).toMatch(/^case "up\+boom" \(n=1\) \[env contract v1\]:$/m);
      expect(s.out()).toMatch(/^R-B verdict \[env contract v1\]: /m);
    });
  });

  describe("7. --allow-drift env means: steering drift within a cohort is reported, not strict", () => {
    it("two digests under the same version are drift; the allow-list downgrades it to a report", async () => {
      const lines = [UP_BOOM_LINE, upBoomLine({ env: "v1:sha256:node-bumped" })].join("\n");
      const strict = sink();
      expect(await runCli(["--strict"], { ...strict, stdin: lines })).toBe(EXIT_STRICT);

      const allowed = sink();
      expect(await runCli(["--strict", "--allow-drift", "env"], { ...allowed, stdin: lines })).toBe(
        EXIT_OK,
      );
      expect(allowed.out()).toMatch(
        /env: v1:sha256:ed62f5285936a0ca, v1:sha256:node-bumped \(allowed by --allow-drift\)/,
      );
    });

    it("it is scoped to the cohort it is reported in and covers no other field there", async () => {
      // v1 clean; v2 has env drift (allow-listed) AND runner drift (not).
      const s = sink();
      const code = await runCli(["--strict", "--allow-drift", "env"], {
        ...s,
        stdin: [
          UP_BOOM_LINE,
          upBoomLine({ env: V2 }),
          upBoomLine({ env: "v2:sha256:0000000000000003", runner: "npm" }),
        ].join("\n"),
      });
      expect(code).toBe(EXIT_STRICT);
      expect(s.out()).toMatch(/env: .* \(allowed by --allow-drift\)/);
      expect(s.out()).toMatch(/runner: pnpm, npm\n/);
    });

    it("is not needed for a split: cross-version digests are never compared", async () => {
      const s = sink();
      const code = await runCli(["--strict"], {
        ...s,
        stdin: [UP_BOOM_LINE, upBoomLine({ env: V2 })].join("\n"),
      });
      expect(code).toBe(EXIT_OK);
      expect(s.out()).not.toMatch(/INPUT DRIFT DETECTED/);
    });
  });

  describe("a real STEERING_ENV_KEYS bump needs no epoch, no --branch, and no --allow-drift", () => {
    const env = {
      npm_config_user_agent: "pnpm/11.3.0 node/v22.0.0 linux/x64",
      NODE_ENV: "test",
      CI: "true",
      PNPM_HOME: "/home/runner/setup-pnpm/node_modules/.bin",
      TMPDIR: "/tmp",
    };
    const current = steeringEnvIdentity(env);
    const next = steeringEnvIdentity(env, {
      keys: [...STEERING_ENV_KEYS, "ZFB_NEXT_STEERING_KEY"],
      version: ENV_IDENTITY_VERSION + 1,
    });

    it("the week the bump merges is a reported split, exit 0, under the bare --strict pass B runs", async () => {
      const s = sink();
      const code = await runCli(["--strict"], {
        ...s,
        stdin: [upBoomLine({ env: current }), upBoomLine({ env: next })].join("\n"),
      });
      expect(code).toBe(EXIT_OK);
      expect(s.out()).toMatch(/!!! ENV IDENTITY CONTRACT VERSION SPLIT !!!/);
      expect(s.out()).toMatch(new RegExp(`identity env: ${current.replace(/[:]/g, "\\$&")}\n`));
      expect(s.out()).toMatch(new RegExp(`identity env: ${next.replace(/[:]/g, "\\$&")}\n`));
    });

    it("a steering change inside the NEW cohort is still caught", async () => {
      const nextBumped = steeringEnvIdentity(
        { ...env, ZFB_NEXT_STEERING_KEY: "on" },
        {
          keys: [...STEERING_ENV_KEYS, "ZFB_NEXT_STEERING_KEY"],
          version: ENV_IDENTITY_VERSION + 1,
        },
      );
      const s = sink();
      const code = await runCli(["--strict"], {
        ...s,
        stdin: [
          upBoomLine({ env: current }),
          upBoomLine({ env: next }),
          upBoomLine({ env: nextBumped }),
        ].join("\n"),
      });
      expect(code).toBe(EXIT_STRICT);
      expect(s.out()).toMatch(/INPUT DRIFT DETECTED/);
    });
  });
});
