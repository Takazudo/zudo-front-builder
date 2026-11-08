import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vite-plus/test";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflow = readFileSync(join(root, ".github/workflows/wind-computed-style.yml"), "utf8");
const STEP = "name: Re-run the v8 build script when its archive is absent";

function stepBlock(marker) {
  const start = workflow.indexOf(`      - ${marker}\n`);
  expect(start, `step "${marker}" not found in wind-computed-style.yml`).toBeGreaterThanOrEqual(0);
  const next = workflow.indexOf("\n      - ", start + 1);
  return workflow.slice(start, next === -1 ? undefined : next);
}

function runBlock(block) {
  const lines = block.split("\n");
  const start = lines.findIndex((line) => /^ {8}run: \|$/.test(line));
  expect(start, "step has no `run: |` block").toBeGreaterThanOrEqual(0);
  const body = [];
  for (const line of lines.slice(start + 1)) {
    if (line.trim() !== "" && !line.startsWith("          ")) break;
    body.push(line.slice(10));
  }
  return body.join("\n");
}

const script = runBlock(stepBlock(STEP));

const temporary = [];
afterEach(() => {
  for (const directory of temporary.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function workspace() {
  const directory = mkdtempSync(join(tmpdir(), "wind-v8-repair-"));
  temporary.push(directory);
  return directory;
}

function touch(base, path) {
  const file = join(base, path);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, `${path}\n`);
}

// Same invocation as the GitHub Actions default `shell: bash`.
function runStep(cwd) {
  const result = spawnSync("bash", ["--noprofile", "--norc", "-eo", "pipefail", "-c", script], {
    cwd,
    encoding: "utf8",
    env: { PATH: "/usr/bin:/bin" },
  });
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function snapshot(base) {
  const entries = [];
  const walk = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        entries.push(`${relative(base, path)}/`);
        walk(path);
      } else {
        entries.push(relative(base, path));
      }
    }
  };
  if (existsSync(base)) walk(base);
  return entries.sort();
}

const V8_LIB = "v8-1a2b3c4d5e6f7081";
const V8_RUN = "v8-9f8e7d6c5b4a3921";
const V8_BUILD_SCRIPT = "v8-0011223344556677";
const SERDE_RUN = "serde-aabbccddeeff0011";

// A cargo profile dir shaped like a rust-cache restore of a v8-dependent build.
function profileFixture(base, profile, { archive }) {
  const p = `target/${profile}`;
  const fp = `${p}/.fingerprint`;
  for (const file of ["lib-v8", "lib-v8.json", "dep-lib-v8"])
    touch(base, `${fp}/${V8_LIB}/${file}`);
  for (const file of [
    "run-build-script-build-script-build",
    "run-build-script-build-script-build.json",
  ]) {
    touch(base, `${fp}/${V8_RUN}/${file}`);
  }
  for (const file of [
    "build-script-build-script-build",
    "build-script-build-script-build.json",
    "dep-build-script-build-script-build",
  ]) {
    touch(base, `${fp}/${V8_BUILD_SCRIPT}/${file}`);
  }
  for (const file of [
    "run-build-script-build-script-build",
    "run-build-script-build-script-build.json",
  ]) {
    touch(base, `${fp}/${SERDE_RUN}/${file}`);
  }
  touch(base, `${p}/build/${V8_RUN}/out/gn_out/obj/librusty_v8.a`);
  touch(base, `${p}/build/${V8_RUN}/output`);
  touch(base, `${p}/build/${V8_BUILD_SCRIPT}/build-script-build`);
  touch(base, `${p}/build/${SERDE_RUN}/output`);
  touch(base, `${p}/deps/libv8-5566778899aabbcc.rlib`);
  if (archive) touch(base, `${p}/librusty_v8.a`);
}

const removedRunFingerprint = (profile) => `target/${profile}/.fingerprint/${V8_RUN}/`;
const withoutRunFingerprint = (tree, profile) =>
  tree.filter((entry) => !entry.startsWith(removedRunFingerprint(profile)));

describe("wind-computed-style v8 build-script re-run step", () => {
  it("extracts a non-trivial script that parses under bash -n", () => {
    expect(script).toContain("librusty_v8.a");
    const result = spawnSync("bash", ["-n", "-c", script], {
      encoding: "utf8",
    });
    expect(result.status, result.stderr).toBe(0);
  });

  it("leaves an intact restore with the archive untouched", () => {
    const base = workspace();
    profileFixture(base, "debug", { archive: true });
    const before = snapshot(base);
    const result = runStep(base);
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).not.toMatch(/re-run/);
    expect(snapshot(base)).toEqual(before);
  });

  it("drops only v8's build-script-run fingerprint when the archive is absent", () => {
    const base = workspace();
    profileFixture(base, "debug", { archive: false });
    const before = snapshot(base);
    const result = runStep(base);
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toBe(`re-run v8 build script: target/debug/.fingerprint/${V8_RUN}/\n`);
    expect(existsSync(join(base, removedRunFingerprint("debug")))).toBe(false);

    const after = snapshot(base);
    expect(after).toEqual(withoutRunFingerprint(before, "debug"));
    // Spell out the survivors the repair must not touch.
    for (const path of [
      `target/debug/.fingerprint/${V8_LIB}/lib-v8`,
      `target/debug/.fingerprint/${V8_BUILD_SCRIPT}/build-script-build-script-build`,
      `target/debug/.fingerprint/${SERDE_RUN}/run-build-script-build-script-build`,
      `target/debug/.fingerprint/${SERDE_RUN}/run-build-script-build-script-build.json`,
      `target/debug/build/${V8_RUN}/out/gn_out/obj/librusty_v8.a`,
      `target/debug/build/${SERDE_RUN}/output`,
      "target/debug/deps/libv8-5566778899aabbcc.rlib",
    ]) {
      expect(after).toContain(path);
    }
  });

  it("is a no-op when target/ does not exist", () => {
    const base = workspace();
    const result = runStep(base);
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toBe("");
    expect(snapshot(base)).toEqual([]);
  });

  it("is a no-op when no v8 fingerprint exists", () => {
    const base = workspace();
    touch(base, `target/debug/.fingerprint/${SERDE_RUN}/run-build-script-build-script-build`);
    touch(base, `target/debug/build/${SERDE_RUN}/output`);
    const before = snapshot(base);
    const result = runStep(base);
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toBe("");
    expect(snapshot(base)).toEqual(before);
  });

  it("is a no-op when no v8 fingerprint has a run-build-script entry", () => {
    const base = workspace();
    touch(base, `target/debug/.fingerprint/${V8_LIB}/lib-v8`);
    touch(base, `target/debug/.fingerprint/${V8_BUILD_SCRIPT}/build-script-build-script-build`);
    touch(base, "target/debug/deps/libv8-5566778899aabbcc.rlib");
    const before = snapshot(base);
    const result = runStep(base);
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toBe("");
    expect(snapshot(base)).toEqual(before);
  });

  it("skips a profile dir without a .fingerprint directory", () => {
    const base = workspace();
    touch(base, "target/tmp/some-scratch-file");
    touch(base, `target/debug/build/${V8_RUN}/output`);
    const before = snapshot(base);
    const result = runStep(base);
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toBe("");
    expect(snapshot(base)).toEqual(before);
  });

  it("repairs a damaged profile and preserves an intact one", () => {
    const base = workspace();
    profileFixture(base, "debug", { archive: false });
    profileFixture(base, "release", { archive: true });
    const before = snapshot(base);
    const result = runStep(base);
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toBe(`re-run v8 build script: target/debug/.fingerprint/${V8_RUN}/\n`);
    expect(snapshot(base)).toEqual(withoutRunFingerprint(before, "debug"));
    expect(existsSync(join(base, removedRunFingerprint("release")))).toBe(true);
  });

  it("is idempotent: a second run logs and changes nothing", () => {
    const base = workspace();
    profileFixture(base, "debug", { archive: false });
    expect(runStep(base).status).toBe(0);
    const afterFirst = snapshot(base);
    const second = runStep(base);
    expect(second.status, second.stderr).toBe(0);
    expect(second.stdout).toBe("");
    expect(snapshot(base)).toEqual(afterFirst);
  });
});
