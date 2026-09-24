import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const script = resolve("scripts/smoke-clean-room.sh");
const fixtures = [];
function scenario({
  responses = ["2.20.3"],
  expected = "",
  installed = "2.20.3",
  runtime = installed,
  cli = installed,
} = {}) {
  const dir = mkdtempSync(join(tmpdir(), "zfb-smoke-identity-"));
  fixtures.push(dir);
  const bin = join(dir, "bin");
  mkdirSync(bin);
  const stub = `#!/usr/bin/env bash
set -euo pipefail
printf '%s %s\\n' "\${0##*/}" "$*" >> "$STUB_LOG"
case "\${0##*/}" in
  npm)
    if [[ "$1" == view ]]; then
      count=$(cat "$STUB_COUNT")
      count=$((count + 1))
      echo "$count" > "$STUB_COUNT"
      IFS=',' read -ra values <<< "$STUB_RESPONSES"
      index=$((count - 1))
      if (( index >= \${#values[@]} )); then index=$((\${#values[@]} - 1)); fi
      if [[ "\${values[$index]}" == ERROR ]]; then exit 1; fi
      if [[ "\${values[$index]}" == ERROR_VERSION ]]; then printf '2.20.3\\n'; exit 1; fi
      printf '%s\\n' "\${values[$index]}"
    fi
    ;;
  npx) mkdir -p smoke-site ;;
  pnpm)
    if [[ "$1" == install ]]; then
      for pkg in zfb zfb-runtime; do
        mkdir -p "node_modules/@takazudo/$pkg"
        version="$STUB_INSTALLED"
        if [[ "$pkg" == zfb-runtime ]]; then version="$STUB_RUNTIME"; fi
        printf '{"version":"%s"}\\n' "$version" > "node_modules/@takazudo/$pkg/package.json"
      done
    elif [[ "$1" == exec ]]; then
      printf 'zfb %s\\n' "$STUB_CLI"
    elif [[ "$1" == build ]]; then
      mkdir -p dist
      printf 'basic-blog Hello, zfb\\n' > dist/index.html
    fi
    ;;
  sleep) ;;
esac
`;
  for (const name of ["npm", "npx", "pnpm", "sleep"]) {
    const path = join(bin, name);
    writeFileSync(path, stub, { mode: 0o755 });
  }
  const log = join(dir, "log");
  const count = join(dir, "count");
  writeFileSync(log, "");
  writeFileSync(count, "0");
  const result = spawnSync("bash", [script], {
    cwd: dir,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      DIST_TAG: "latest",
      EXPECTED_VERSION: expected,
      STUB_LOG: log,
      STUB_COUNT: count,
      STUB_RESPONSES: responses.join(","),
      STUB_INSTALLED: installed,
      STUB_RUNTIME: runtime,
      STUB_CLI: cli,
    },
  });
  return { ...result, log: readFileSync(log, "utf8"), count: Number(readFileSync(count, "utf8")) };
}
afterEach(() => {
  for (const dir of fixtures.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("real clean-room script with offline registry and CLI stubs", () => {
  it("waits for the intended release, then pins all probes and npx", () => {
    const run = scenario({ responses: ["2.20.2", "2.20.3", "2.20.4"], expected: "2.20.3" });
    expect(run.status).toBe(0);
    expect(run.count).toBe(2);
    expect(run.log.match(/npm pack --dry-run @takazudo\/zfb-[^ ]+@2\.20\.3/g)).toHaveLength(5);
    expect(run.log).toContain("npx --yes create-zfb@2.20.3 smoke-site");
    expect(run.log).not.toContain("@latest smoke-site");
  });
  it("fails a permanently stale release channel before install", () => {
    const run = scenario({ responses: ["2.20.2"], expected: "2.20.3" });
    expect(run.status).toBe(1);
    expect(run.count).toBe(6);
    expect(run.log).not.toContain("npx ");
  });
  it.each(["", "ERROR", "ERROR_VERSION", "not-a-version"])(
    "rejects unavailable or invalid registry response %s",
    (response) => {
      const run = scenario({ responses: [response], expected: "2.20.3" });
      expect(run.status).toBe(1);
      expect(run.log).not.toContain("npx ");
    },
  );
  it("rejects unexpected installed package version", () => {
    const run = scenario({ expected: "2.20.3", installed: "2.20.2" });
    expect(run.status).toBe(1);
    expect(run.stderr).toContain("Installed @takazudo/zfb@2.20.2");
    expect(run.log).not.toContain("pnpm build");
  });
  it("rejects unexpected CLI version", () => {
    const run = scenario({ expected: "2.20.3", cli: "2.20.2" });
    expect(run.status).toBe(1);
    expect(run.stderr).toContain("Installed zfb CLI reports");
  });
  it("rejects unexpected runtime version", () => {
    const run = scenario({ expected: "2.20.3", runtime: "2.20.2" });
    expect(run.status).toBe(1);
    expect(run.stderr).toContain("Installed @takazudo/zfb-runtime@2.20.2");
  });
  it("pins a scheduled moving channel after one resolution", () => {
    const run = scenario({ responses: ["2.20.3", "2.20.4"] });
    expect(run.status).toBe(0);
    expect(run.count).toBe(1);
    expect(run.log).toContain("npx --yes create-zfb@2.20.3 smoke-site");
  });
  it("wires normal and recovery smoke to the verified tag and skips dry runs", () => {
    const release = readFileSync(resolve(".github/workflows/release.yml"), "utf8");
    const reusable = readFileSync(
      resolve(".github/workflows/reusable-smoke-clean-room.yml"),
      "utf8",
    );
    expect(release).toContain("release_version: ${{ steps.verify.outputs.release_version }}");
    expect(release).toContain('echo "release_version=$EXPECTED_VERSION" >> "$GITHUB_OUTPUT"');
    expect(release).toMatch(
      /smoke-clean-room:[\s\S]*?needs: \[publish, release-context\][\s\S]*?if: \$\{\{ inputs\.dry_run != true \}\}[\s\S]*?expected_version: \$\{\{ needs\.release-context\.outputs\.release_version \}\}/,
    );
    expect(reusable).toContain("EXPECTED_VERSION: ${{ inputs.expected_version }}");
  });
});
