import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { afterEach, describe, expect, it } from "vite-plus/test";

const workflow = readFileSync(".github/workflows/island-size-darwin.yml", "utf8");
const health = readFileSync(".github/workflows/health.yml", "utf8");
const job = workflow.slice(workflow.indexOf("\n  rust-linux:"));
const dirs = [];
function script(name) {
  const step = job.split(`      - name: ${name}\n`)[1]?.split("\n      - ")[0];
  expect(step, name).toBeDefined();
  return step
    .split("        run: |\n")[1]
    .split("\n")
    .map((line) => line.slice(10))
    .join("\n");
}
function fixture() {
  const cwd = mkdtempSync(join(tmpdir(), "rust-workflow-"));
  dirs.push(cwd);
  const bin = join(cwd, "bin");
  const evidence = join(cwd, "evidence");
  mkdirSync(bin);
  mkdirSync(evidence);
  return {
    cwd,
    bin,
    evidence,
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      RUST_EVIDENCE_DIR: evidence,
      GITHUB_STEP_SUMMARY: join(cwd, "summary"),
    },
  };
}
function execute(body, f, env = {}) {
  return spawnSync("bash", ["--noprofile", "--norc", "-eo", "pipefail", "-c", body], {
    cwd: f.cwd,
    env: { ...f.env, ...env },
    encoding: "utf8",
  });
}
function stub(f, name, text) {
  writeFileSync(join(f.bin, name), `#!/usr/bin/env bash\n${text}\n`, { mode: 0o755 });
}
function cargoStub(f) {
  stub(
    f,
    "cargo",
    `
    printf '%s\\n' "$*" >> "$RUST_EVIDENCE_DIR/calls"
    if [[ "$1 $2" == 'nextest run' && "\${NO_JUNIT:-}" != 1 ]]; then
      mkdir -p target/nextest/ci
      printf '<testsuites tests="1"><testsuite tests="1"><testcase name="executed"/></testsuite></testsuites>\\n' > target/nextest/ci/junit.xml
    fi
    if [[ "$1" == clippy && "\${FAIL_CLIPPY:-}" == 1 ]]; then exit 42; fi
    if [[ "$1 $2" == 'nextest run' && "\${EMPTY_TESTS:-}" == 1 ]]; then exit 4; fi
    echo completed
  `,
  );
}
const labels = [
  "inventory",
  "clippy",
  "workspace",
  "doctests",
  "md-extras",
  "md-extras-clippy",
  "no-v8",
  "jsx-no-v8",
  "env-esbuild",
];
afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("independent Linux Rust verification", () => {
  it("isolates the new mode from native measurements and every CSS carrier", () => {
    expect(workflow).toContain("          - rust-linux\n");
    expect(workflow).toContain(
      "if: inputs.platform == 'darwin-arm64' || inputs.platform == 'both'",
    );
    expect(workflow).toContain("if: inputs.platform == 'linux-x64' || inputs.platform == 'both'");
    expect(workflow).toContain("if: ${{ inputs.css_carriers && inputs.platform != 'rust-linux' }}");
    expect(job).toContain("if: inputs.platform == 'rust-linux'");
    expect(job).toContain("runs-on: ubuntu-24.04");
    expect(job).not.toMatch(
      /macos|self-hosted|issues: write|contents: write|secrets:|continue-on-error/,
    );
    expect(job).not.toContain("check-budget.mjs");
    expect(job).not.toContain("test:workspace");
    expect(workflow).toContain("permissions:\n  contents: read");
  });

  it("reuses supported hosted cleanup before requiring headroom", () => {
    const name = "Free disk space for V8 build (issue #687)";
    const expected = health.split(`      - name: ${name}\n`)[1].split("\n      - ")[0].trim();
    expect(job.split(`      - name: ${name}\n`)[1].split("\n      - ")[0].trim()).toBe(expected);
    expect(job.indexOf(name)).toBeLessThan(
      job.indexOf("Record exact source and require disk headroom"),
    );
    const f = fixture();
    stub(f, "sudo", 'echo "$*" >> "$RUST_EVIDENCE_DIR/cleanup"');
    stub(f, "df", 'printf "Avail\\n%s\\n" "$FREE_BYTES"');
    expect(execute(script(name), f, { FREE_BYTES: "80000000000" }).status).toBe(0);
    expect(existsSync(join(f.evidence, "cleanup"))).toBe(false);
    expect(execute(script(name), f, { FREE_BYTES: "10000000000" }).status).toBe(0);
    expect(readFileSync(join(f.evidence, "cleanup"), "utf8")).toContain(
      "docker image prune --all --force",
    );
    expect(execute(script(name), f, { FREE_BYTES: "invalid" }).status).not.toBe(0);
  });

  it.each(["", "main", "bda633b", "A".repeat(40), `${"a".repeat(40)}; echo injected`])(
    "rejects nonimmutable source %j",
    (source) => {
      const f = fixture();
      expect(
        execute(script("Require immutable source and Rust-only dispatch"), f, {
          SOURCE_SHA: source,
          CSS_CARRIERS: "false",
          CSS_WINDOWS_ONLY: "false",
        }).status,
      ).not.toBe(0);
    },
  );

  it("accepts a full SHA but rejects either CSS flag", () => {
    const f = fixture();
    const env = { SOURCE_SHA: "a".repeat(40), CSS_CARRIERS: "false", CSS_WINDOWS_ONLY: "false" };
    const body = script("Require immutable source and Rust-only dispatch");
    expect(execute(body, f, env).status).toBe(0);
    for (const key of ["CSS_CARRIERS", "CSS_WINDOWS_ONLY"])
      expect(execute(body, f, { ...env, [key]: "true" }).status).not.toBe(0);
  });

  it("binds checkout to the requested SHA and records control/source identities separately", () => {
    const f = fixture();
    const sha = "a".repeat(40);
    stub(f, "git", 'printf "%s\\n" "$ACTUAL_SHA"');
    stub(f, "df", 'printf "Avail\\n80000000000\\n"');
    expect(job).toContain("ref: ${{ inputs.ref }}");
    const body = script("Record exact source and require disk headroom");
    const env = {
      SOURCE_SHA: sha,
      ACTUAL_SHA: "b".repeat(40),
      GITHUB_WORKFLOW_SHA: "c".repeat(40),
    };
    expect(execute(body, f, env).status).not.toBe(0);
    expect(execute(body, f, { ...env, ACTUAL_SHA: sha }).status).toBe(0);
    expect(readFileSync(join(f.evidence, "source.txt"), "utf8")).toContain(sha);
    expect(readFileSync(join(f.evidence, "workflow-sha.txt"), "utf8")).toContain("c".repeat(40));
  });

  it("checks disk again after dependency/cache setup before compiling", () => {
    const f = fixture();
    stub(f, "df", 'printf "Avail\\n1000000\\n"');
    cargoStub(f);
    expect(execute(script("Build all test targets once"), f).status).not.toBe(0);
    expect(existsSync(join(f.evidence, "calls"))).toBe(false);
    expect(job.indexOf("Build all test targets once")).toBeGreaterThan(
      job.indexOf("Swatinem/rust-cache@"),
    );
  });

  it("keeps health's complete workspace env filter and zero-retry, fail-empty test commands", () => {
    const body = script("Independent Rust checks, zero retries");
    const expected = health
      .match(/--run-ignored ignored-only -E '([\s\S]*?)'/)[1]
      .replace(/\s+/g, " ")
      .trim();
    const actual = body.match(/--run-ignored ignored-only -E '([^']*)'/)[1];
    expect(actual).toBe(expected);
    expect(body.match(/--profile ci --retries 0 --no-tests=fail/g)).toHaveLength(3);
    expect(body).toContain("cargo test --locked --workspace --doc");
    expect(body).toContain(
      "cargo clippy --locked -p zfb-md-extras --features test-utils --all-targets -- -D warnings",
    );
    expect(job).toContain(
      "ZFB_ESBUILD_BIN: ${{ github.workspace }}/crates/zfb/binaries/esbuild/esbuild",
    );
  });

  it("retains every first result and continues after a genuine failure without retry", () => {
    const f = fixture();
    cargoStub(f);
    const result = execute(script("Independent Rust checks, zero retries"), f, {
      FAIL_CLIPPY: "1",
    });
    expect(result.status).toBe(1);
    expect(readFileSync(join(f.evidence, "calls"), "utf8").trim().split("\n")).toHaveLength(
      labels.length,
    );
    for (const label of labels) expect(existsSync(join(f.evidence, `${label}.exit`))).toBe(true);
    expect(readFileSync(join(f.evidence, "clippy.exit"), "utf8").trim()).toBe("42");
    expect(readFileSync(join(f.evidence, "env-esbuild.exit"), "utf8").trim()).toBe("0");
    for (const label of ["workspace", "md-extras", "env-esbuild"])
      expect(existsSync(join(f.evidence, `${label}.junit.xml`))).toBe(true);
  });

  it.each([{ NO_JUNIT: "1" }, { EMPTY_TESTS: "1" }])(
    "fails missing or empty execution evidence %j",
    (env) => {
      const f = fixture();
      cargoStub(f);
      mkdirSync(join(f.cwd, "target/nextest/ci"), { recursive: true });
      writeFileSync(join(f.cwd, "target/nextest/ci/junit.xml"), "stale report");
      expect(execute(script("Independent Rust checks, zero retries"), f, env).status).not.toBe(0);
      if (env.NO_JUNIT) expect(existsSync(join(f.evidence, "workspace.junit.xml"))).toBe(false);
    },
  );

  it("treats log capture errors as failures even when the command succeeds", () => {
    const f = fixture();
    cargoStub(f);
    stub(f, "tee", "cat >/dev/null; exit 23");
    expect(execute(script("Independent Rust checks, zero retries"), f).status).toBe(1);
    expect(readFileSync(join(f.evidence, "workspace.exit"), "utf8").trim()).toBe("0");
    expect(readFileSync(join(f.evidence, "workspace.log-exit"), "utf8").trim()).toBe("23");
  });

  it("reports missing checks instead of treating a prerequisite failure as green", () => {
    const f = fixture();
    const body = script("Report missing or failed coverage");
    expect(execute(body, f).status).toBe(1);
    expect(readFileSync(join(f.evidence, "summary.md"), "utf8")).toContain(
      "| workspace | missing |",
    );
    expect(readFileSync(join(f.evidence, "summary.md"), "utf8")).toContain(
      "live admission is not evaluated",
    );
    expect(job).toMatch(/Report missing or failed coverage\n\s+if: always\(\)/);
    expect(job).toMatch(/Retain first results\n\s+if: always\(\)/);
    expect(job).toContain("${{ env.RUST_EVIDENCE_DIR }}/");
  });

  it("reports success only for completed checks with retained reports", () => {
    const f = fixture();
    cargoStub(f);
    expect(execute(script("Independent Rust checks, zero retries"), f).status).toBe(0);
    for (const suffix of ["exit", "log-exit"])
      writeFileSync(join(f.evidence, `build.${suffix}`), "0\n");
    expect(execute(script("Report missing or failed coverage"), f).status).toBe(0);
    rmSync(join(f.evidence, "env-esbuild.junit.xml"));
    expect(execute(script("Report missing or failed coverage"), f).status).toBe(1);
  });
});
