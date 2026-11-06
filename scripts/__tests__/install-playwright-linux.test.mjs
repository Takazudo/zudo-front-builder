import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import {
  buildOfficialRoute,
  parseArgs,
  parseDryRun,
  provision,
  runBounded,
} from "../install-playwright-linux.mjs";

// Verbatim shape of a GitHub-hosted Ubuntu 24.04 runner after
// actions/runner-images configure-apt-sources.sh (both stanzas on the mirror list).
const RUNNER_SOURCES = `## Ubuntu distribution repository
Types: deb
URIs: mirror+file:/etc/apt/apt-mirrors.txt
Suites: noble noble-updates noble-backports
Components: main universe restricted multiverse
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg

## Ubuntu security updates.
Types: deb
URIs: mirror+file:/etc/apt/apt-mirrors.txt
Suites: noble-security
Components: main universe restricted multiverse
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
`;
const RUNNER_MIRRORS =
  "http://azure.archive.ubuntu.com/ubuntu/\tpriority:1\nhttps://archive.ubuntu.com/ubuntu/\tpriority:2\nhttps://security.ubuntu.com/ubuntu/\tpriority:3\n";
const THIRD_PARTY =
  "deb [arch=amd64,arm64,armhf signed-by=/etc/apt/keyrings/microsoft.gpg] https://packages.microsoft.com/repos/azure-cli/ noble main\n";

const LOCKED = "1.61.0";
const NODE = "/usr/bin/node";
const CLI = "/repo/node_modules/playwright-core/cli.js";

const satisfied = { exitCode: 0, stdout: "All system dependencies are installed.\n" };
const missing = (...names) => ({
  exitCode: 1,
  stdout: `Missing system dependencies (${names.length}):\n${names.map((name) => `  ${name}`).join("\n")}\n`,
});
const success = { exitCode: 0 };

function aptVerb(args) {
  const index = args.indexOf("apt-get");
  if (index < 0) return null;
  const rest = args.slice(index + 1);
  if (rest[0] === "update") return "update";
  if (rest.includes("--download-only")) return "download";
  if (rest.includes("--no-download")) return "install";
  return "other";
}

// Scripted collaborators. `script` maps a call kind to a queue of outcomes;
// each outcome may carry `durationMs`, which advances the fake clock.
function fakeDeps({ script = {}, files = {}, processes = [[]], playwright = {}, existing } = {}) {
  let clock = 1_000_000;
  const calls = [];
  const written = new Map();
  const processQueue = [...processes];
  const queues = Object.fromEntries(
    Object.entries(script).map(([key, value]) => [key, [...value]]),
  );
  const take = (kind) => {
    const queue = queues[kind];
    if (!queue || queue.length === 0) throw new Error(`unexpected ${kind} call`);
    return queue.length === 1 ? queue[0] : queue.shift();
  };
  const executables =
    playwright.executables ?? ((engine) => [{ name: engine, path: `/browsers/${engine}` }]);
  return {
    calls,
    written,
    get clock() {
      return clock;
    },
    deps: {
      now: () => clock,
      env: { GITHUB_SHA: "a".repeat(40), ImageOS: "ubuntu24", ImageVersion: "20261101.360.1" },
      isRoot: false,
      nodePath: NODE,
      gitHead: () => "a".repeat(40),
      sleep: async (ms) => {
        clock += ms;
      },
      listProcesses: () => (processQueue.length > 1 ? processQueue.shift() : processQueue[0]),
      readText: (file) => files[file] ?? (written.has(file) ? written.get(file) : null),
      listDir: (dir) =>
        Object.keys(files)
          .filter((file) => file.startsWith(`${dir}/`))
          .map((file) => file.slice(dir.length + 1))
          .sort(),
      exists: existing ?? (() => true),
      makeTempDir: () => "/tmp/zfb-apt-route-test",
      mkdir: () => {},
      writeText: (file, text) => written.set(file, text),
      appendText: () => {},
      playwright: () => ({
        coreVersion: LOCKED,
        testVersion: LOCKED,
        playwrightVersion: LOCKED,
        lockVersions: [LOCKED],
        hostPlatform: "ubuntu24.04-x64",
        officiallySupported: true,
        cli: CLI,
        coreDir: "/repo/node_modules/playwright-core",
        executables,
        ...playwright,
      }),
      run: async (spec) => {
        let kind;
        if (spec.command === NODE && spec.args.includes("--dry-run")) kind = "dryRun";
        else if (spec.command === NODE && spec.args[1] === "install") kind = "browsers";
        else if (spec.command === NODE && spec.args[0] === "-e") kind = "launch";
        else kind = aptVerb(spec.args) ?? "unknown";
        const outcome = take(kind);
        calls.push({ kind, spec, at: clock });
        clock += outcome.durationMs ?? 1000;
        return {
          signal: null,
          timedOut: false,
          durationMs: outcome.durationMs ?? 1000,
          tail: "",
          stdout: "",
          ...outcome,
        };
      },
    },
  };
}

const options = (overrides = {}) => ({
  label: "health-chromium",
  engines: ["chromium"],
  acquireTimeoutMs: 240_000,
  installTimeoutMs: 300_000,
  browserTimeoutMs: 300_000,
  totalTimeoutMs: 1_800_000,
  receiptDir: "/receipts",
  quiescenceMs: 30_000,
  ...overrides,
});

const runnerFiles = {
  "/etc/apt/sources.list.d/ubuntu.sources": RUNNER_SOURCES,
  "/etc/apt/sources.list.d/azure-cli.list": THIRD_PARTY,
  "/etc/apt/apt-mirrors.txt": RUNNER_MIRRORS,
};

const kinds = (fake) => fake.calls.map((call) => call.kind);
const timeoutSeconds = (call) =>
  Number(call.spec.args.find((arg) => /^\d+s$/.test(arg)).slice(0, -1));

describe("argument contract", () => {
  it("selects engines in canonical order and requires explicit bounds", () => {
    const parsed = parseArgs([
      "--label",
      "health-wind-full",
      "--engines",
      "webkit,chromium",
      "--engines",
      "firefox",
      "--acquire-timeout",
      "360",
      "--browser-timeout",
      "420",
      "--total-timeout",
      "1200",
    ]);
    expect(parsed.engines).toEqual(["chromium", "firefox", "webkit"]);
    expect(parsed.acquireTimeoutMs).toBe(360_000);
    expect(parsed.installTimeoutMs).toBe(300_000);
    expect(parsed.totalTimeoutMs).toBe(1_200_000);
    expect(() =>
      parseArgs([
        "--label",
        "x",
        "--engines",
        "chrome",
        "--acquire-timeout",
        "1",
        "--browser-timeout",
        "1",
        "--total-timeout",
        "9",
      ]),
    ).toThrow(/unknown engine: chrome/);
    expect(() =>
      parseArgs([
        "--label",
        "x",
        "--engines",
        "chromium,chromium",
        "--acquire-timeout",
        "1",
        "--browser-timeout",
        "1",
        "--total-timeout",
        "9",
      ]),
    ).toThrow(/twice/);
    expect(() =>
      parseArgs(["--label", "x", "--engines", "chromium", "--browser-timeout", "1"]),
    ).toThrow(/--acquire-timeout/);
    expect(() =>
      parseArgs([
        "--engines",
        "chromium",
        "--acquire-timeout",
        "1",
        "--browser-timeout",
        "1",
        "--total-timeout",
        "9",
      ]),
    ).toThrow(/--label/);
  });
});

describe("Playwright dependency report", () => {
  it("accepts only the exact satisfied or missing-package forms", () => {
    expect(parseDryRun(satisfied)).toEqual({ satisfied: true, packages: [] });
    expect(parseDryRun(missing("libgtk-4-1", "libavif16", "xvfb"))).toEqual({
      satisfied: false,
      packages: ["libgtk-4-1", "libavif16", "xvfb"],
    });
  });

  it.each([
    [
      "simulation failure",
      {
        exitCode: 1,
        stdout:
          "Failed to install browser dependencies\nError: 'apt-get install -s' exited with code 100:\nE: Unable to locate package libgtk-4-1\n",
      },
    ],
    [
      "unknown platform warning",
      {
        exitCode: 0,
        stdout: "Cannot install dependencies for ubuntu30.04-x64 with Playwright 1.61.0!\n",
      },
    ],
    ["count mismatch", { exitCode: 1, stdout: "Missing system dependencies (2):\n  libgtk-4-1\n" }],
    [
      "malformed package line",
      { exitCode: 1, stdout: "Missing system dependencies (1):\n  libgtk-4-1 extra\n" },
    ],
    [
      "satisfied text with failure exit",
      { exitCode: 1, stdout: "All system dependencies are installed.\n" },
    ],
    [
      "extra output",
      {
        exitCode: 0,
        stdout:
          "BEWARE: your OS is not officially supported\nAll system dependencies are installed.\n",
      },
    ],
  ])("rejects %s", (_name, outcome) => {
    expect(() => parseDryRun(outcome)).toThrow(/unrecognised Playwright dependency report/);
  });
});

describe("official-archive route", () => {
  it("reroutes the runner mirror stanzas and keeps every other field and source", () => {
    const route = buildOfficialRoute({
      sourcesList: "# Ubuntu sources have moved to /etc/apt/sources.list.d/ubuntu.sources\n",
      parts: [
        { name: "ubuntu.sources", content: RUNNER_SOURCES },
        { name: "azure-cli.list", content: THIRD_PARTY },
        { name: "ignored.save", content: "deb http://example.invalid/ x y\n" },
      ],
      mirrorList: RUNNER_MIRRORS,
    });
    expect(route.rewritten).toBe(2);
    const ubuntu = route.parts.find((part) => part.name === "ubuntu.sources").content;
    expect(ubuntu).toContain(
      "Types: deb\nURIs: https://archive.ubuntu.com/ubuntu/\nSuites: noble noble-updates noble-backports\nComponents: main universe restricted multiverse\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg",
    );
    expect(ubuntu).toContain("URIs: https://security.ubuntu.com/ubuntu/\nSuites: noble-security\n");
    expect(ubuntu).not.toContain("mirror+file");
    expect(ubuntu.match(/Signed-By:/g)).toHaveLength(2);
    expect(route.parts.find((part) => part.name === "azure-cli.list").content).toBe(THIRD_PARTY);
    expect(route.parts.map((part) => part.name)).not.toContain("ignored.save");
  });

  it("splits a combined stanza so security suites stay on the security host", () => {
    const route = buildOfficialRoute({
      sourcesList: null,
      parts: [
        {
          name: "ubuntu.sources",
          content:
            "Types: deb\nURIs: http://azure.archive.ubuntu.com/ubuntu/\nSuites: resolute resolute-updates resolute-security\nComponents: main universe\nArchitectures: amd64\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n",
        },
      ],
      mirrorList: null,
    });
    const content = route.parts[0].content;
    expect(content).toContain(
      "URIs: https://archive.ubuntu.com/ubuntu/\nSuites: resolute resolute-updates\nComponents: main universe\nArchitectures: amd64\nSigned-By:",
    );
    expect(content).toContain(
      "URIs: https://security.ubuntu.com/ubuntu/\nSuites: resolute-security\nComponents: main universe\nArchitectures: amd64\nSigned-By:",
    );
  });

  it("rewrites legacy one-line sources and keeps their options", () => {
    const route = buildOfficialRoute({
      sourcesList:
        "deb [arch=amd64] mirror+file:/etc/apt/apt-mirrors.txt jammy main restricted\ndeb mirror+file:/etc/apt/apt-mirrors.txt jammy-security main\n# deb-src http://archive.ubuntu.com/ubuntu jammy main\n",
      parts: [],
      mirrorList: RUNNER_MIRRORS,
    });
    expect(route.sourcesList).toBe(
      "deb [arch=amd64] https://archive.ubuntu.com/ubuntu/ jammy main restricted\ndeb https://security.ubuntu.com/ubuntu/ jammy-security main\n# deb-src http://archive.ubuntu.com/ubuntu jammy main\n",
    );
  });

  it.each([
    [
      "a non-official mirror list",
      { mirrorList: "http://mirror.example.invalid/ubuntu/\tpriority:1\n" },
      /non-official mirror/,
    ],
    ["a missing mirror list", { mirrorList: null }, /missing/],
  ])("refuses %s", (_name, override, pattern) => {
    expect(() =>
      buildOfficialRoute({
        sourcesList: null,
        parts: [{ name: "ubuntu.sources", content: RUNNER_SOURCES }],
        ...override,
      }),
    ).toThrow(pattern);
  });

  it("refuses sources that disable authentication or have nothing to reroute", () => {
    expect(() =>
      buildOfficialRoute({
        sourcesList: "deb [trusted=yes] http://archive.ubuntu.com/ubuntu/ noble main\n",
        parts: [],
        mirrorList: null,
      }),
    ).toThrow(/disables authentication/);
    expect(() =>
      buildOfficialRoute({
        sourcesList: null,
        parts: [
          {
            name: "u.sources",
            content:
              "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu/\nSuites: noble\nComponents: main\nTrusted: yes\n",
          },
        ],
        mirrorList: null,
      }),
    ).toThrow(/disables authentication/);
    expect(() =>
      buildOfficialRoute({
        sourcesList: null,
        parts: [
          { name: "azure-cli.list", content: THIRD_PARTY },
          {
            name: "off.sources",
            content:
              "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu/\nSuites: noble\nComponents: main\nEnabled: no\n",
          },
        ],
        mirrorList: null,
      }),
    ).toThrow(/no allowlisted Ubuntu source/);
  });
});

describe("provisioning flow", () => {
  it("reuses already-satisfied dependencies and installs exactly the selected browsers", async () => {
    const fake = fakeDeps({
      script: { dryRun: [satisfied], browsers: [success], launch: [success] },
    });
    const { receipt, exitCode, receiptPath } = await provision(
      options({ label: "health-wind-full", engines: ["chromium", "firefox", "webkit"] }),
      fake.deps,
    );
    expect(exitCode).toBe(0);
    expect(receiptPath).toBe("/receipts/health-wind-full.json");
    expect(kinds(fake)).toEqual(["dryRun", "browsers", "launch", "launch", "launch"]);
    expect(fake.calls[0].spec.args).toEqual([
      CLI,
      "install-deps",
      "--dry-run",
      "chromium",
      "firefox",
      "webkit",
    ]);
    expect(fake.calls[1].spec.args).toEqual([CLI, "install", "chromium", "firefox", "webkit"]);
    expect(fake.calls.slice(2).map((call) => call.spec.args[1])).toEqual([
      expect.stringContaining("const { chromium }"),
      expect.stringContaining("const { firefox }"),
      expect.stringContaining("const { webkit }"),
    ]);
    expect(receipt.dependencies.reused).toBe(true);
    expect(receipt.result).toBe("success");
    expect(JSON.parse(fake.written.get(receiptPath)).result).toBe("success");
  });

  it("acquires completely before installing from the acquired archives", async () => {
    const fake = fakeDeps({
      script: {
        dryRun: [
          { exitCode: 1, stdout: "Failed to install browser dependencies\n" },
          missing("libnss3", "xvfb"),
          satisfied,
        ],
        update: [success],
        download: [success],
        install: [success],
        browsers: [success],
        launch: [success],
      },
    });
    const { receipt, exitCode } = await provision(options(), fake.deps);
    expect(exitCode).toBe(0);
    expect(kinds(fake)).toEqual([
      "dryRun",
      "update",
      "dryRun",
      "download",
      "install",
      "dryRun",
      "browsers",
      "launch",
    ]);
    const download = fake.calls[3].spec.args;
    const install = fake.calls[4].spec.args;
    expect(download.slice(0, 2)).toEqual(["-n", "--"]);
    expect(download.slice(download.indexOf("apt-get"))).toEqual([
      "apt-get",
      "install",
      "-y",
      "--download-only",
      "--no-install-recommends",
      "libnss3",
      "xvfb",
    ]);
    expect(install.slice(install.indexOf("apt-get"))).toEqual([
      "apt-get",
      "install",
      "-y",
      "--no-download",
      "--no-install-recommends",
      "libnss3",
      "xvfb",
    ]);
    expect(install.some((arg) => arg.startsWith("APT_CONFIG="))).toBe(false);
    expect(receipt.dependencies).toMatchObject({
      reused: false,
      route: "runner-default",
      packages: ["libnss3", "xvfb"],
    });
    expect(receipt.firstTransportFailure).toBeNull();
    expect(receipt.phases.map((phase) => phase.stage)).toEqual([
      "probe",
      "acquire:metadata",
      "acquire:resolve",
      "acquire:download",
      "install",
      "validate",
      "browsers",
      "launch:chromium",
    ]);
  });

  it("bounds the whole acquisition attempt by elapsed time, not per command", async () => {
    const fake = fakeDeps({
      script: {
        dryRun: [missing("libnss3"), missing("libnss3"), satisfied],
        update: [{ exitCode: 0, durationMs: 100_000 }],
        download: [success],
        install: [success],
        browsers: [success],
        launch: [success],
      },
    });
    await provision(options({ acquireTimeoutMs: 240_000 }), fake.deps);
    const [update, download] = fake.calls.filter(
      (call) => call.kind === "update" || call.kind === "download",
    );
    expect(timeoutSeconds(update)).toBe(240);
    expect(timeoutSeconds(download)).toBeLessThanOrEqual(140);
    expect(timeoutSeconds(download)).toBeGreaterThan(130);
  });

  it("stops at the total deadline instead of starting the fallback", async () => {
    const fake = fakeDeps({
      files: runnerFiles,
      script: {
        dryRun: [missing("libnss3")],
        update: [success],
        download: [{ exitCode: 124, durationMs: 240_000 }],
      },
    });
    const { receipt, exitCode } = await provision(options({ totalTimeoutMs: 260_000 }), fake.deps);
    expect(exitCode).toBe(1);
    expect(kinds(fake)).toEqual(["dryRun", "update", "dryRun", "download"]);
    expect(receipt.error).toMatch(/total provisioning deadline reached/);
    expect(receipt.firstTransportFailure.error).toBe(
      "package download reached the acquisition deadline",
    );
    // Clipped to the total: 260 s minus the 20 s receipt reserve.
    expect(timeoutSeconds(fake.calls[1])).toBeLessThanOrEqual(240);
  });

  it("retries once on the official archive after a slow download reaches its deadline", async () => {
    const fake = fakeDeps({
      files: runnerFiles,
      processes: [[{ pid: 41, args: "/usr/lib/apt/methods/http" }], []],
      script: {
        dryRun: [missing("libnss3"), missing("libnss3"), missing("libnss3"), satisfied],
        update: [success],
        download: [{ exitCode: 124, durationMs: 230_000, tail: "Get:15 ... [8998 kB]" }, success],
        install: [success],
        browsers: [success],
        launch: [success],
      },
    });
    const { receipt, exitCode } = await provision(options(), fake.deps);
    expect(exitCode).toBe(0);
    expect(kinds(fake)).toEqual([
      "dryRun",
      "update",
      "dryRun",
      "download",
      "update",
      "dryRun",
      "download",
      "install",
      "dryRun",
      "browsers",
      "launch",
    ]);
    const firstDownloadEnd = fake.calls[3].at + 230_000;
    expect(fake.calls[4].at).toBeGreaterThan(firstDownloadEnd);
    const aptConfig = "APT_CONFIG=/tmp/zfb-apt-route-test/apt.conf";
    for (const call of fake.calls.slice(4, 9)) {
      if (call.kind === "dryRun")
        expect(call.spec.env.APT_CONFIG).toBe("/tmp/zfb-apt-route-test/apt.conf");
      else expect(call.spec.args).toContain(aptConfig);
    }
    expect(fake.calls[0].spec.env.APT_CONFIG).toBeUndefined();
    expect(fake.written.get("/tmp/zfb-apt-route-test/sources.list.d/ubuntu.sources")).toContain(
      "URIs: https://archive.ubuntu.com/ubuntu/",
    );
    expect(fake.written.get("/tmp/zfb-apt-route-test/sources.list.d/azure-cli.list")).toBe(
      THIRD_PARTY,
    );
    expect(fake.written.get("/tmp/zfb-apt-route-test/apt.conf")).toContain(
      'Dir::State::Lists "/tmp/zfb-apt-route-test/lists";',
    );
    expect(receipt.firstTransportFailure).toEqual({
      attempt: 1,
      route: "runner-default",
      error: "package download reached the acquisition deadline",
    });
    const downloads = receipt.phases.filter((phase) => phase.stage === "acquire:download");
    expect(downloads).toMatchObject([
      { attempt: 1, route: "runner-default", exitCode: 124, outputTail: "Get:15 ... [8998 kB]" },
      { attempt: 2, route: "official-archive", exitCode: 0 },
    ]);
    expect(receipt.dependencies.route).toBe("official-archive");
  });

  it("fails after the single fallback is exhausted, without installing", async () => {
    const fake = fakeDeps({
      files: runnerFiles,
      script: {
        dryRun: [missing("libnss3")],
        update: [success, { exitCode: 124, durationMs: 240_000 }],
        download: [{ exitCode: 100 }],
      },
    });
    const { receipt, exitCode } = await provision(options(), fake.deps);
    expect(exitCode).toBe(1);
    expect(kinds(fake)).toEqual(["dryRun", "update", "dryRun", "download", "update"]);
    expect(receipt.failedStage).toBe("acquire");
    expect(receipt.error).toBe(
      "apt-get install --download-only exited 100; retry: apt-get update reached the acquisition deadline",
    );
    expect(receipt.firstTransportFailure.error).toBe("apt-get install --download-only exited 100");
  });

  it("refuses an overlapping retry while acquisition processes survive", async () => {
    const fake = fakeDeps({
      files: runnerFiles,
      processes: [[{ pid: 77, args: "apt-get install -y --download-only libnss3" }]],
      script: {
        dryRun: [missing("libnss3")],
        update: [success],
        download: [{ exitCode: 124, durationMs: 240_000 }],
      },
    });
    const { receipt, exitCode } = await provision(options(), fake.deps);
    expect(exitCode).toBe(1);
    expect(kinds(fake)).toEqual(["dryRun", "update", "dryRun", "download"]);
    expect(receipt.failedStage).toBe("acquire");
    expect(receipt.error).toMatch(/refusing an overlapping retry: 77 apt-get install/);
  });

  it("propagates an installation failure without a transport retry", async () => {
    const fake = fakeDeps({
      files: runnerFiles,
      script: {
        dryRun: [missing("libnss3")],
        update: [success],
        download: [success],
        install: [{ exitCode: 100, tail: "dpkg: error processing" }],
      },
    });
    const { receipt, exitCode } = await provision(options(), fake.deps);
    expect(exitCode).toBe(1);
    expect(kinds(fake)).toEqual(["dryRun", "update", "dryRun", "download", "install"]);
    expect(receipt.failedStage).toBe("install");
    expect(receipt.firstTransportFailure).toBeNull();
  });

  it("fails clearly when resolution after metadata is unrecognised, without a retry", async () => {
    const fake = fakeDeps({
      files: runnerFiles,
      script: {
        dryRun: [
          missing("libnss3"),
          {
            exitCode: 0,
            stdout: "Cannot install dependencies for ubuntu24.04-x64 with Playwright 1.61.0!\n",
          },
        ],
        update: [success],
      },
    });
    const { receipt, exitCode } = await provision(options(), fake.deps);
    expect(exitCode).toBe(1);
    expect(kinds(fake)).toEqual(["dryRun", "update", "dryRun"]);
    expect(receipt.failedStage).toBe("acquire:resolve");
  });

  it("does not report success when the post-install check still finds missing packages", async () => {
    const fake = fakeDeps({
      script: {
        dryRun: [missing("libnss3"), missing("libnss3"), missing("libnss3")],
        update: [success],
        download: [success],
        install: [success],
      },
    });
    const { receipt, exitCode } = await provision(options(), fake.deps);
    expect(exitCode).toBe(1);
    expect(receipt.failedStage).toBe("validate");
    expect(kinds(fake)).not.toContain("browsers");
  });

  it.each([
    ["a version that differs from the lockfile", { coreVersion: "1.62.0" }],
    ["two locked versions", { lockVersions: ["1.61.0", "1.60.0"] }],
    ["an unsupported platform", { officiallySupported: false, hostPlatform: "ubuntu24.04-x64" }],
    ["a non-Ubuntu platform", { hostPlatform: "debian12-x64" }],
  ])("refuses %s before touching APT", async (_name, playwright) => {
    const fake = fakeDeps({ playwright });
    const { receipt, exitCode } = await provision(options(), fake.deps);
    expect(exitCode).toBe(1);
    expect(receipt.failedStage).toBe("identity");
    expect(fake.calls).toEqual([]);
  });

  it("fails on a browser download failure or a missing executable", async () => {
    const failedDownload = fakeDeps({
      script: { dryRun: [satisfied], browsers: [{ exitCode: 1 }] },
    });
    expect((await provision(options(), failedDownload.deps)).receipt.failedStage).toBe("browsers");

    const absent = fakeDeps({
      script: { dryRun: [satisfied], browsers: [success] },
      existing: (file) => file !== "/browsers/chromium",
    });
    const { receipt, exitCode } = await provision(options(), absent.deps);
    expect(exitCode).toBe(1);
    expect(receipt.failedStage).toBe("verify");
    expect(kinds(absent)).not.toContain("launch");
  });

  it("fails when a browser cannot launch", async () => {
    const fake = fakeDeps({
      script: { dryRun: [satisfied], browsers: [success], launch: [{ exitCode: 1 }] },
    });
    const { receipt, exitCode } = await provision(options(), fake.deps);
    expect(exitCode).toBe(1);
    expect(receipt.failedStage).toBe("launch");
  });

  it("records source, runner and Playwright identity in the receipt", async () => {
    const fake = fakeDeps({
      script: { dryRun: [satisfied], browsers: [success], launch: [success] },
    });
    const { receipt } = await provision(options(), fake.deps);
    expect(receipt).toMatchObject({
      schema: "zfb-playwright-linux-provisioning/v1",
      engines: ["chromium"],
      source: { githubSha: "a".repeat(40), checkoutHead: "a".repeat(40) },
      runner: { imageOs: "ubuntu24", imageVersion: "20261101.360.1" },
      playwright: { version: LOCKED, hostPlatform: "ubuntu24.04-x64" },
      limits: { acquireTimeoutMs: 240_000, maxAcquisitionAttempts: 2 },
    });
  });
});

describe("bounded subprocess", () => {
  const directories = [];
  afterEach(() => {
    for (const directory of directories.splice(0))
      rmSync(directory, { recursive: true, force: true });
  });

  it("terminates the whole process group at the deadline before settling", async () => {
    const directory = mkdtempSync(join(tmpdir(), "bounded-run-"));
    directories.push(directory);
    const script = join(directory, "slow.sh");
    writeFileSync(script, '#!/bin/sh\nsleep 60 &\necho "child $!"\nwait\n', { mode: 0o755 });
    const result = await runBounded({
      command: script,
      timeoutMs: 300,
      killGraceMs: 200,
      echo: false,
    });
    expect(result.timedOut).toBe(true);
    // Settling waits for every holder of the output pipes, so a surviving
    // grandchild would hold this open for its full 60 seconds.
    expect(result.durationMs).toBeLessThan(10_000);
    const childPid = Number(result.stdout.match(/child (\d+)/)[1]);
    // Gone, or a zombie awaiting a reaper: either way it no longer runs.
    const state = existsSync(`/proc/${childPid}/stat`)
      ? readFileSync(`/proc/${childPid}/stat`, "utf8").replace(/^.*\) /, "")[0]
      : "gone";
    expect(["gone", "Z", "X"]).toContain(state);
  });

  it("reports a missing command as a failure, never a success", async () => {
    const result = await runBounded({
      command: "/nonexistent/apt-get",
      timeoutMs: 1000,
      echo: false,
    });
    expect(result.exitCode).not.toBe(0);
  });
});
