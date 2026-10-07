import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";

const script = resolve("scripts/smoke-clean-room.sh");
const platforms = [
  "@takazudo/zfb-darwin-arm64",
  "@takazudo/zfb-darwin-x64",
  "@takazudo/zfb-linux-arm64-gnu",
  "@takazudo/zfb-linux-x64-gnu",
  "@takazudo/zfb-win32-x64-msvc",
];
const fixtures = [];

function scenario({
  responses = ["2.20.3"],
  metadataResponses = [],
  curlResponses = [],
  expected = "",
  installed = "2.20.3",
  runtime = installed,
  cli = installed,
  deadlineSeconds = "900",
  versionAdvanceSeconds = "0",
  metadataAdvanceSeconds = "0",
  curlAdvanceSeconds = "0",
} = {}) {
  const dir = mkdtempSync(join(tmpdir(), "zfb-smoke-identity-"));
  fixtures.push(dir);
  const bin = join(dir, "bin");
  mkdirSync(bin);
  const stub = `#!/usr/bin/env bash
set -euo pipefail
name=\${0##*/}
if [[ "$name" != now ]]; then printf '%s %s\\n' "$name" "$*" >> "$STUB_LOG"; fi
pick_response() {
  local script="$1" count="$2" fallback="$3"
  if [[ -z "$script" ]]; then printf '%s' "$fallback"; return; fi
  IFS=',' read -ra values <<< "$script"
  local index=$((count - 1))
  if (( index >= \${#values[@]} )); then index=$((\${#values[@]} - 1)); fi
  printf '%s' "\${values[$index]}"
}
advance_clock() {
  local seconds="$1" current
  if (( seconds > 0 )); then
    current=$(cat "$STUB_CLOCK")
    echo $((current + seconds)) > "$STUB_CLOCK"
  fi
}
case "$name" in
  npm)
    if [[ "$1" == view && "$3" == version ]]; then
      count=$(cat "$STUB_COUNT")
      count=$((count + 1))
      echo "$count" > "$STUB_COUNT"
      response=$(pick_response "$STUB_RESPONSES" "$count" "2.20.3")
      advance_clock "$STUB_VERSION_ADVANCE_SECONDS"
      case "$response" in
        ERROR) exit 1 ;;
        ERROR_VERSION) printf '2.20.3\\n'; exit 1 ;;
        EMPTY) exit 0 ;;
        *) printf '%s\\n' "$response" ;;
      esac
    elif [[ "$1" == view && "$3" == dist.tarball ]]; then
      count=$(cat "$STUB_METADATA_COUNT")
      count=$((count + 1))
      echo "$count" > "$STUB_METADATA_COUNT"
      response=$(pick_response "$STUB_METADATA_RESPONSES" "$count" "OK")
      advance_clock "$STUB_METADATA_ADVANCE_SECONDS"
      case "$response" in
        ERROR) exit 1 ;;
        EMPTY) exit 0 ;;
        OK) printf 'https://registry.test/tarball/%s.tgz\\n' "$count" ;;
        *) printf '%s\\n' "$response" ;;
      esac
    fi
    ;;
  curl)
    count=$(cat "$STUB_CURL_COUNT")
    count=$((count + 1))
    echo "$count" > "$STUB_CURL_COUNT"
    response=$(pick_response "$STUB_CURL_RESPONSES" "$count" "0:206")
    advance_clock "$STUB_CURL_ADVANCE_SECONDS"
    exit_code=\${response%%:*}
    http_code=\${response#*:}
    printf '%s' "$http_code"
    printf 'curl-result %s %s\\n' "$exit_code" "$http_code" >> "$STUB_LOG"
    exit "$exit_code"
    ;;
  now) cat "$STUB_CLOCK" ;;
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
  sleep)
    current=$(cat "$STUB_CLOCK")
    echo $((current + $1)) > "$STUB_CLOCK"
    ;;
esac
`;
  for (const name of ["npm", "curl", "now", "npx", "pnpm", "sleep"]) {
    writeFileSync(join(bin, name), stub, { mode: 0o755 });
  }
  const log = join(dir, "log");
  const count = join(dir, "count");
  const metadataCount = join(dir, "metadata-count");
  const curlCount = join(dir, "curl-count");
  const clock = join(dir, "clock");
  writeFileSync(log, "");
  writeFileSync(count, "0");
  writeFileSync(metadataCount, "0");
  writeFileSync(curlCount, "0");
  writeFileSync(clock, "1000");
  const encodeResponses = (items) => items.map((item) => (item === "" ? "EMPTY" : item)).join(",");
  const result = spawnSync("bash", [script], {
    cwd: dir,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      TMPDIR: dir,
      DIST_TAG: "latest",
      EXPECTED_VERSION: expected,
      SMOKE_PROPAGATION_DEADLINE_SECONDS: deadlineSeconds,
      SMOKE_NOW_CMD: join(bin, "now"),
      STUB_LOG: log,
      STUB_COUNT: count,
      STUB_METADATA_COUNT: metadataCount,
      STUB_CURL_COUNT: curlCount,
      STUB_CLOCK: clock,
      STUB_RESPONSES: encodeResponses(responses),
      STUB_METADATA_RESPONSES: encodeResponses(metadataResponses),
      STUB_CURL_RESPONSES: encodeResponses(curlResponses),
      STUB_VERSION_ADVANCE_SECONDS: versionAdvanceSeconds,
      STUB_METADATA_ADVANCE_SECONDS: metadataAdvanceSeconds,
      STUB_CURL_ADVANCE_SECONDS: curlAdvanceSeconds,
      STUB_INSTALLED: installed,
      STUB_RUNTIME: runtime,
      STUB_CLI: cli,
    },
  });
  return {
    ...result,
    log: readFileSync(log, "utf8"),
    count: Number(readFileSync(count, "utf8")),
    metadataCount: Number(readFileSync(metadataCount, "utf8")),
    curlCount: Number(readFileSync(curlCount, "utf8")),
    elapsed: Number(readFileSync(clock, "utf8")) - 1000,
  };
}

afterEach(() => {
  for (const dir of fixtures.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("real clean-room script with offline registry and CLI stubs", () => {
  it("waits for the intended release, then pins all probes and npx", () => {
    const run = scenario({ responses: ["2.20.2", "2.20.3", "2.20.4"], expected: "2.20.3" });
    expect(run.status).toBe(0);
    expect(run.count).toBe(2);
    expect(run.log.match(/npm view @takazudo\/zfb-[^ ]+@2\.20\.3 dist\.tarball/g)).toHaveLength(5);
    expect(
      run.log.match(
        /curl -fsSL --connect-timeout 10 --max-time 60 -r 0-0 -o \/dev\/null -w %\{http_code\} https:\/\/registry\.test\/tarball\/\d+\.tgz/g,
      ),
    ).toHaveLength(5);
    expect(run.log).toContain("npx --yes create-zfb@2.20.3 smoke-site");
    expect(run.log).not.toContain("npm pack");
    expect(run.log).not.toContain("--cache");
  });

  it("fails a permanently stale release channel before install", () => {
    const run = scenario({ responses: ["2.20.2"], expected: "2.20.3", deadlineSeconds: "30" });
    expect(run.status).toBe(1);
    expect(run.count).toBe(3);
    expect(run.stderr).toContain("before the shared propagation deadline");
    expect(run.log).not.toContain("npx ");
  });

  it.each(["", "ERROR", "ERROR_VERSION", "not-a-version"])(
    "rejects unavailable or invalid registry response %s",
    (response) => {
      const run = scenario({ responses: [response], expected: "2.20.3", deadlineSeconds: "10" });
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

  it("fails at the shared deadline when package metadata never resolves", () => {
    const run = scenario({ metadataResponses: ["ERROR"], deadlineSeconds: "20" });
    expect(run.status).toBe(1);
    expect(run.metadataCount).toBe(2);
    expect(run.curlCount).toBe(0);
    expect(run.stderr).toContain("metadata unavailable (elapsed 20s, last: npm view failed)");
  });

  it("waits through a tarball 404 and accepts a later HTTP 200", () => {
    const run = scenario({ curlResponses: ["22:404", "0:200"], deadlineSeconds: "20" });
    expect(run.status).toBe(0);
    expect(run.curlCount).toBe(6);
    expect(run.log).toContain("curl-result 22 404");
    expect(run.log).toContain("curl-result 0 200");
    expect(
      run.log.match(
        /curl -fsSL --connect-timeout 10 --max-time 60 -r 0-0 -o \/dev\/null -w %\{http_code\}/g,
      ),
    ).toHaveLength(6);
    expect(run.log).not.toContain("npm pack");
    expect(run.log).not.toContain("--cache");
    expect(run.stdout).toContain("tarball unavailable (elapsed 0s, last HTTP 404)");
    expect(run.stdout).toContain(
      "tarball probe passed for @takazudo/zfb-darwin-arm64@2.20.3 (HTTP 200)",
    );
  });

  it("fails at the deadline when metadata exists but the tarball never fetches", () => {
    const run = scenario({ curlResponses: ["22:404"], deadlineSeconds: "20" });
    expect(run.status).toBe(1);
    expect(run.curlCount).toBe(2);
    expect(run.stderr).toContain(
      "metadata present, tarball unavailable (elapsed 20s, last HTTP 404)",
    );
  });

  it("shares one deadline across version resolution and delayed platform metadata", () => {
    const run = scenario({
      responses: ["ERROR", "2.20.3"],
      metadataResponses: ["ERROR", "OK", "ERROR", "OK"],
      deadlineSeconds: "25",
    });
    expect(run.status).toBe(1);
    expect(run.count).toBe(2);
    expect(run.metadataCount).toBe(3);
    expect(run.elapsed).toBe(25);
    expect(run.log).toContain("npm view @takazudo/zfb-darwin-arm64@2.20.3 dist.tarball");
    expect(run.log).toContain("npm view @takazudo/zfb-darwin-x64@2.20.3 dist.tarball");
    expect(run.stderr).toContain(
      "@takazudo/zfb-darwin-x64@2.20.3 metadata unavailable (elapsed 25s",
    );
    expect(run.log).not.toContain("npx ");
  });

  it("rejects an empty dist.tarball value", () => {
    const run = scenario({ metadataResponses: ["EMPTY"], deadlineSeconds: "10" });
    expect(run.status).toBe(1);
    expect(run.metadataCount).toBe(1);
    expect(run.stderr).toContain("metadata unavailable (elapsed 10s, last: empty dist.tarball)");
  });

  it("checks all five platform tarballs before scaffolding", () => {
    const run = scenario();
    expect(run.status).toBe(0);
    const firstScaffold = run.log.indexOf("npx --yes create-zfb@2.20.3 smoke-site");
    expect(firstScaffold).toBeGreaterThan(0);
    for (const pkg of platforms) {
      const metadata = run.log.indexOf(`npm view ${pkg}@2.20.3 dist.tarball`);
      const probe = run.log.indexOf("curl -fsSL", metadata);
      expect(metadata).toBeGreaterThanOrEqual(0);
      expect(probe).toBeGreaterThan(metadata);
      expect(probe).toBeLessThan(firstScaffold);
    }
  });

  it("rejects version and tarball responses that finish after the shared deadline", () => {
    const versionRun = scenario({ versionAdvanceSeconds: "6", deadlineSeconds: "5" });
    expect(versionRun.status).toBe(1);
    expect(versionRun.count).toBe(1);
    expect(versionRun.metadataCount).toBe(0);
    expect(versionRun.stderr).toContain(
      "version metadata arrived after the shared propagation deadline",
    );

    const tarballRun = scenario({ curlAdvanceSeconds: "6", deadlineSeconds: "5" });
    expect(tarballRun.status).toBe(1);
    expect(tarballRun.curlCount).toBe(1);
    expect(tarballRun.stderr).toContain(
      "tarball probe finished after the shared propagation deadline",
    );
    expect(tarballRun.log).not.toContain("npx ");
  });

  it("rejects package metadata that resolves after the shared deadline", () => {
    const run = scenario({ metadataAdvanceSeconds: "6", deadlineSeconds: "5" });
    expect(run.status).toBe(1);
    expect(run.metadataCount).toBe(1);
    expect(run.curlCount).toBe(0);
    expect(run.stderr).toContain("metadata resolved after the shared propagation deadline");
  });

  it.each(["0.5", "", "0", "-1"])(
    "rejects invalid shared deadline %s before registry access",
    (deadlineSeconds) => {
      const run = scenario({ deadlineSeconds });
      expect(run.status).toBe(1);
      expect(run.stderr).toContain("SMOKE_PROPAGATION_DEADLINE_SECONDS must be");
      expect(run.count).toBe(0);
      expect(run.metadataCount).toBe(0);
    },
  );
});
