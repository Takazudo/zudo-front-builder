// The md-wasm exact-size gate only holds when the job that measures
// shipped-sizes.json (health.yml wasm-md) and the job that rebuilds the
// shipped .wasm files (release.yml build-wasm-md) compile with the same rustc.
// v6.1.0 measured on 1.99.1 and built on 1.100.0 under floating `stable`.
import { execFileSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";
import { scriptsDir } from "./helpers/darwin-fixture.mjs";

const repoRoot = join(scriptsDir, "..");
const read = (path) => readFileSync(join(repoRoot, path), "utf8");
const pin = read("crates/zfb-md-wasm/rust-toolchain-pin").trim();
const SELECTOR = "scripts/select-md-wasm-rust-toolchain.sh";

const stepIndex = (steps, predicate) => steps.findIndex(predicate);
const runs = (step, text) => typeof step.run === "string" && step.run.includes(text);

describe("md-wasm Rust toolchain pin", () => {
  it("is an exact rustc version", () => {
    expect(pin).toMatch(/^1\.\d+\.\d+$/);
  });

  it("matches the rustc shipped-sizes.json was measured with", () => {
    // A pin bump needs a fresh CI measurement on the new compiler (RELEASING.md A4).
    const manifest = JSON.parse(read("crates/zfb-md-wasm/shipped-sizes.json"));
    expect(manifest.measuredWithRustc).toBe(pin);
  });

  for (const [workflow, jobName, buildStep] of [
    [
      ".github/workflows/health.yml",
      "wasm-md",
      "Build four @takazudo/zfb-md-wasm artifacts (timed)",
    ],
    [
      ".github/workflows/release.yml",
      "build-wasm-md",
      "Build four @takazudo/zfb-md-wasm artifacts (timed)",
    ],
  ]) {
    it(`${workflow} ${jobName} selects the pin before caching and building`, () => {
      const steps = parse(read(workflow)).jobs[jobName].steps;
      const select = stepIndex(steps, (s) => runs(s, SELECTOR));
      const cache = stepIndex(steps, (s) =>
        String(s.uses ?? "").startsWith("Swatinem/rust-cache@"),
      );
      const build = stepIndex(steps, (s) => s.name === buildStep);
      const firstCargo = stepIndex(steps, (s) => runs(s, "cargo "));
      expect(select).toBeGreaterThanOrEqual(0);
      expect(select).toBeLessThan(cache);
      expect(select).toBeLessThan(build);
      if (firstCargo >= 0) expect(select).toBeLessThan(firstCargo);
      // A bare target add on floating stable may only survive as the legacy fallback.
      for (const step of steps) {
        if (runs(step, "rustup target add")) expect(runs(step, SELECTOR)).toBe(true);
      }
    });
  }
});

describe(SELECTOR, () => {
  const tempDirs = [];
  afterEach(() => {
    for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  function fakeToolchain(rustcVersion) {
    const dir = mkdtempSync(join(tmpdir(), "md-wasm-pin-"));
    tempDirs.push(dir);
    const log = join(dir, "rustup.log");
    writeFileSync(join(dir, "rustup"), `#!/bin/sh\necho "$@" >> "${log}"\n`);
    writeFileSync(
      join(dir, "rustc"),
      `#!/bin/sh\necho "rustc ${rustcVersion} (fake 2026-01-01) toolchain=$RUSTUP_TOOLCHAIN"\n`,
    );
    chmodSync(join(dir, "rustup"), 0o755);
    chmodSync(join(dir, "rustc"), 0o755);
    const githubEnv = join(dir, "github-env");
    writeFileSync(githubEnv, "");
    return { dir, log, githubEnv };
  }

  function run(fake, args = []) {
    return execFileSync("bash", [join(repoRoot, SELECTOR), ...args], {
      env: { PATH: `${fake.dir}:/usr/bin:/bin`, GITHUB_ENV: fake.githubEnv },
      encoding: "utf8",
      stdio: "pipe",
    });
  }

  it("installs the pin with wasm32 and exports RUSTUP_TOOLCHAIN", () => {
    const fake = fakeToolchain(pin);
    run(fake, ["clippy"]);
    expect(readFileSync(fake.log, "utf8").trim()).toBe(
      `toolchain install ${pin} --profile minimal --no-self-update --target wasm32-unknown-unknown --component clippy`,
    );
    expect(readFileSync(fake.githubEnv, "utf8")).toBe(`RUSTUP_TOOLCHAIN=${pin}\n`);
  });

  it("fails and exports nothing when rustc is not the pinned version", () => {
    const fake = fakeToolchain("1.0.0");
    let failure;
    try {
      run(fake);
    } catch (error) {
      failure = error;
    }
    expect(failure?.status).toBe(1);
    expect(String(failure?.stderr)).toMatch(
      /expected rustc \S+ from .*rust-toolchain-pin, got: rustc 1\.0\.0/,
    );
    expect(readFileSync(fake.githubEnv, "utf8")).toBe("");
  });
});
