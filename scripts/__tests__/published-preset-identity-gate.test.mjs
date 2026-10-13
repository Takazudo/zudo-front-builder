import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";
import {
  FATAL_CONSOLE_TYPES,
  consoleOracleFailures,
  isFatalConsoleMessage,
} from "../../tests/published-preset-identity/console-policy.mjs";
import { skipRecord } from "../../tests/published-preset-identity/record-skip.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflow = readFileSync(join(root, ".github/workflows/health.yml"), "utf8");

function stepBlock(marker) {
  const start = workflow.indexOf(`      - ${marker}\n`);
  expect(start, `step "${marker}" not found in health.yml`).toBeGreaterThanOrEqual(0);
  const next = workflow.indexOf("\n      - ", start + 1);
  return workflow.slice(start, next === -1 ? undefined : next);
}

function stepIf(block) {
  const match = block.match(/^ {8}if: (.+)$/m);
  if (!match) return null;
  return match[1].replace(/^\$\{\{\s*/, "").replace(/\s*\}\}$/, "");
}

// The subset of the GitHub Actions expression language health.yml uses here.
function evaluate(expression, ctx) {
  const tokens = expression.match(/\|\||&&|==|!=|!|\(|\)|'[^']*'|[A-Za-z_][\w.]*/g);
  let pos = 0;
  const peek = () => tokens[pos];
  const take = () => tokens[pos++];
  const primary = () => {
    const token = take();
    if (token === "(") {
      const value = or();
      expect(take()).toBe(")");
      return value;
    }
    if (token.startsWith("'")) return token.slice(1, -1);
    if (peek() === "(") {
      take();
      expect(take()).toBe(")");
      expect(ctx.functions, `unsupported function ${token}`).toHaveProperty(token);
      return ctx.functions[token];
    }
    const step = token.match(/^steps\.(\w+)\.outcome$/);
    expect(step, `unsupported operand ${token}`).not.toBeNull();
    return ctx.outcomes[step[1]];
  };
  const unary = () => (peek() === "!" ? (take(), !unary()) : comparison());
  const comparison = () => {
    const left = primary();
    if (peek() === "==" || peek() === "!=") {
      const op = take();
      const right = primary();
      return op === "==" ? left === right : left !== right;
    }
    return left;
  };
  const and = () => {
    let value = unary();
    while (peek() === "&&") {
      take();
      value = unary() && value;
    }
    return value;
  };
  const or = () => {
    let value = and();
    while (peek() === "||") {
      take();
      value = or() || value;
    }
    return value;
  };
  const value = or();
  expect(pos).toBe(tokens.length);
  return Boolean(value);
}

function runs(block, { outcomes, failed = false, cancelled = false }) {
  let expression = stepIf(block) ?? "success()";
  if (!/\b(?:always|cancelled|failure|success)\(\)/.test(expression)) {
    expression = `success() && (${expression})`;
  }
  return evaluate(expression, {
    outcomes,
    functions: {
      always: true,
      cancelled,
      failure: failed && !cancelled,
      success: !failed && !cancelled,
    },
  });
}

const fixture = stepBlock("name: Published-preset runtime identity fixture (#3331)");
const record = stepBlock("name: Record published-preset fixture prerequisite skip (#4177)");
const upload = stepBlock("name: Upload published-preset identity evidence (#3331)");

function simulate(state) {
  const fixtureRuns = runs(fixture, state);
  const outcomes = { ...state.outcomes, published_preset: fixtureRuns ? "success" : "skipped" };
  return {
    fixture: fixtureRuns,
    record: runs(record, { ...state, outcomes }),
    upload: runs(upload, { ...state, outcomes }),
  };
}

describe("published-preset fixture gate in health.yml (#4177)", () => {
  it("names both prerequisite steps the gate depends on", () => {
    expect(stepBlock("id: build_binary")).toContain("run: cargo build --workspace --all-targets");
    expect(stepBlock("name: Install Chromium for scanner boundary acceptance")).toMatch(
      /^ {8}id: chromium$/m,
    );
    expect(fixture).toMatch(/^ {8}id: published_preset$/m);
    expect(fixture).toContain("ZFB_BIN: ${{ github.workspace }}/target/debug/zfb");
  });

  it("keeps the fixture a hard failure: no continue-on-error, no always()", () => {
    for (const block of [fixture, record]) expect(block).not.toContain("continue-on-error");
    expect(stepIf(fixture)).not.toContain("always()");
  });

  it("runs when its prerequisites succeeded", () => {
    const outcomes = { build_binary: "success", chromium: "success" };
    expect(simulate({ outcomes })).toEqual({ fixture: true, record: false, upload: true });
  });

  it("still runs after an unrelated earlier failure", () => {
    const outcomes = { build_binary: "success", chromium: "success" };
    expect(simulate({ outcomes, failed: true })).toEqual({
      fixture: true,
      record: false,
      upload: true,
    });
  });

  it.each([
    ["the binary build failed", { build_binary: "failure", chromium: "success" }],
    ["the binary build was skipped", { build_binary: "skipped", chromium: "success" }],
    ["the Chromium install failed", { build_binary: "success", chromium: "failure" }],
  ])("records an explicit skip when %s", (_label, outcomes) => {
    expect(simulate({ outcomes, failed: true })).toEqual({
      fixture: false,
      record: true,
      upload: true,
    });
  });

  it("records an explicit skip on cancellation", () => {
    const outcomes = { build_binary: "success", chromium: "success" };
    expect(simulate({ outcomes, cancelled: true })).toEqual({
      fixture: false,
      record: true,
      upload: true,
    });
  });
});

describe("published-preset skip record (#4177)", () => {
  it("names the unmet prerequisite and is never a pass", () => {
    const value = skipRecord({
      outcomes: { build_binary: "failure", chromium: "success" },
      jobStatus: "failure",
    });
    expect(value).toMatchObject({
      status: "skipped",
      reason: "prerequisite-not-met",
      unmet: ["build_binary"],
      prerequisites: { build_binary: "failure", chromium: "success" },
    });
  });

  it("distinguishes cancellation", () => {
    const value = skipRecord({
      outcomes: { build_binary: "success", chromium: "success" },
      jobStatus: "cancelled",
    });
    expect(value).toMatchObject({ status: "skipped", reason: "cancelled", unmet: [] });
  });

  it("refuses to record a skip the gate should not have produced", () => {
    expect(() =>
      skipRecord({
        outcomes: { build_binary: "success", chromium: "success" },
        jobStatus: "failure",
      }),
    ).toThrow(/every prerequisite succeeded/);
  });
});

describe("published-preset positive console oracle (#4177)", () => {
  it("treats warnings as fatal alongside errors", () => {
    expect(FATAL_CONSOLE_TYPES).toEqual(["error", "warning"]);
    expect(isFatalConsoleMessage("warning")).toBe(true);
    expect(isFatalConsoleMessage("error")).toBe(true);
    for (const type of ["log", "info", "debug"]) expect(isFatalConsoleMessage(type)).toBe(false);
  });

  it("fails on a warning, an error or a page error", () => {
    expect(consoleOracleFailures({ console: ["warning: deprecated"], pageErrors: [] })).toEqual([
      "console errors/warnings: warning: deprecated",
    ]);
    expect(consoleOracleFailures({ console: ["error: boom"], pageErrors: [] })).toHaveLength(1);
    expect(consoleOracleFailures({ console: [], pageErrors: ["TypeError"] })).toEqual([
      "page errors: TypeError",
    ]);
    expect(consoleOracleFailures({ console: [], pageErrors: [] })).toEqual([]);
  });

  it("is wired into the harness with no allowlist", () => {
    const harness = readFileSync(join(root, "tests/published-preset-identity/run.mjs"), "utf8");
    expect(harness).toContain("isFatalConsoleMessage(message.type())");
    expect(harness).toContain("failures.push(...consoleOracleFailures(facts));");
    expect(harness).not.toMatch(/allowlist|ignoreWarning/i);
  });
});
