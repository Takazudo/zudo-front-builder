import { execFile } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { afterEach, describe, expect, it } from "vitest";
import { makeFieldNotesSource, sourceTreeHashes } from "../lib/fieldnotes.mjs";
import { freezeHarness, unfreezeForRemoval } from "../lib/frozen.mjs";
import { superviseController } from "../lib/session-supervisor.mjs";
import { HARNESS_DIR, readJson } from "../lib/util.mjs";
import { makeFakeArchive } from "./fieldnotes-fake.mjs";

// Every test here awaits a real child process (the generator, git, a frozen cli.mjs), so this
// file runs in the scripts-subprocess project (vitest.config.mjs SUBPROCESS_SUITES).
const execFileAsync = promisify(execFile);
const FIXTURE = join(HARNESS_DIR, "fixtures", "field-notes");
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-sub-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) {
    const d = tmps.pop();
    try {
      unfreezeForRemoval(d);
    } catch {
      // not frozen
    }
    rmSync(d, { recursive: true, force: true });
  }
});

describe("make-source runs the pinned generator", () => {
  it("copies apps/shared/scripts, runs the pinned generator at the scale and checks every hash", async () => {
    const a = await makeFakeArchive(tmp(), FIXTURE);
    a.scales.byCount["100"].generatorArgs = "40";
    const first = await makeFieldNotesSource({
      fixtureDir: FIXTURE,
      archiveDir: a.archive,
      scale: 100,
      outDir: join(a.root, "probe"),
      scales: a.scales,
    });
    expect(first.generator.status).toBe(0);
    expect(first.ok).toBe(false);
    const decl = Object.fromEntries(first.checks.map((c) => [c.name.split(":")[1], c.actual]));
    a.scales.byCount["100"] = { ...a.scales.byCount["100"], ...decl };
    const out = join(a.root, "src-100");
    const r = await makeFieldNotesSource({
      fixtureDir: FIXTURE,
      archiveDir: a.archive,
      scale: 100,
      outDir: out,
      scales: a.scales,
    });
    expect(r.ok).toBe(true);
    expect(r.scale).toBe(100);
    expect(readJson(join(out, "content", "manifest.json")).articleCount).toBe(40);
    expect(r.source.datasetSha256).toBe(sourceTreeHashes(out, a.manifest).datasetSha256);
    expect(existsSync(join(a.archive, "content"))).toBe(false);
    await expect(
      makeFieldNotesSource({
        fixtureDir: FIXTURE,
        archiveDir: a.archive,
        scale: 100,
        outDir: out,
        scales: a.scales,
      }),
    ).rejects.toThrow(/already exists/);
  });

  it("a generator that exits non-zero is reported, with no hash checks", async () => {
    const a = await makeFakeArchive(tmp(), FIXTURE);
    a.scales.byCount["100"].generatorArgs = "40";
    const r = await makeFieldNotesSource({
      fixtureDir: FIXTURE,
      archiveDir: a.archive,
      scale: 100,
      outDir: join(a.root, "src"),
      scales: a.scales,
      nodePath: join(a.root, "no-such-node"),
    });
    expect(r.ok).toBe(false);
    expect(r.generator.status).not.toBe(0);
    expect(r.checks).toEqual([]);
  });
});

describe("freeze refuses a dirty harness dir (real git)", () => {
  it("refuses to freeze a harness dir with uncommitted changes", async () => {
    const repo = tmp();
    const git = (...a) =>
      execFileAsync("git", ["-c", "user.name=t", "-c", "user.email=t@example.invalid", ...a], {
        cwd: repo,
      });
    await git("init", "-q");
    mkdirSync(join(repo, "h"));
    writeFileSync(join(repo, "h", "cli.mjs"), "");
    await git("add", ".");
    await git("commit", "-qm", "x");
    writeFileSync(join(repo, "h", "new.mjs"), "");
    expect(() =>
      freezeHarness({ outDir: join(repo, "..", "never"), harnessDir: join(repo, "h") }),
    ).toThrow(/uncommitted changes/);
  });
});

describe("a frozen copy's own cli.mjs", () => {
  it("verify-identity from a frozen copy with a broken playwright link fails that check", async () => {
    const out = join(tmp(), "frozen");
    const r = freezeHarness({
      outDir: out,
      allowDirty: true,
      capabilityImpl: () => ({
        repo: "/fake/repo",
        sha: "f".repeat(40),
        harnessDirDirty: false,
        harnessDirPatchSha256: null,
        harnessDirUntracked: [],
      }),
    });
    rmSync(r.stamp.playwright.link);
    symlinkSync(tmpdir(), r.stamp.playwright.link, "dir");
    const profile = JSON.parse(
      readFileSync(join(HARNESS_DIR, "profiles", "tiny-internal-a0.example.json"), "utf8"),
    );
    profile.fixture.dir = join(r.dir, "fixtures", "tiny-two-article");
    profile.outDir = join(tmp(), "out");
    const pPath = join(tmp(), "p.json");
    writeFileSync(pPath, JSON.stringify(profile));
    const res = await execFileAsync(process.execPath, [
      join(r.dir, "cli.mjs"),
      "verify-identity",
      "--profile",
      pPath,
    ]).then(
      () => null,
      (error) => ({ code: error.code, out: JSON.parse(error.stdout) }),
    );
    expect(res.code).toBe(3);
    expect(res.out.checks.find((c) => c.name === "harness-frozen-copy-intact").ok).toBe(true);
    expect(res.out.checks.find((c) => c.name === "harness-frozen-playwright-link")).toMatchObject({
      ok: false,
      actual: tmpdir(),
    });
  });
});

describe("independent session watchdog", () => {
  it.each([
    ["stalled preparation", "", 900],
    ["stalled finalization", 'process.send({ type: "finalizing" });', 10000],
  ])("terminates its own blocked controller during %s", async (label, ready, wallMs) => {
    const worker = join(tmp(), "controller.mjs");
    writeFileSync(worker, ready + "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0);");
    const started = Number(process.hrtime.bigint() / 1_000_000n);
    const messages = [];
    const reports = [];
    expect(
      await superviseController({
        worker,
        args: { label },
        deadline: started + wallMs,
        terminalMs: 250,
        stderr: (s) => messages.push(s),
        recordFailure: (r) => reports.push(r),
      }),
    ).toBe(8);
    expect(messages.join("\n")).toContain("terminal deadline reached");
    expect(reports).toHaveLength(1);
    expect(reports[0]).toMatchObject({
      attemptedControllerSignal: "SIGKILL",
      terminalBudgetMs: 250,
      phase: "finalizing",
      deadlineMonotonicMs: started + wallMs,
    });
    expect(Number(process.hrtime.bigint() / 1_000_000n) - started).toBeLessThan(6000);
  });

  it("rejects a controller exiting zero without the final completion message", async () => {
    const worker = join(tmp(), "exited.mjs");
    writeFileSync(worker, "process.exitCode = 0;");
    const reports = [];
    const code = await superviseController({
      worker,
      args: { label: "unexpected-exit" },
      deadline: Number(process.hrtime.bigint() / 1_000_000n) + 5_000,
      terminalMs: 200,
      stderr() {},
      recordFailure: (r) => reports.push(r),
    });
    expect(code).toBe(8);
    expect(reports).toHaveLength(1);
    expect(reports[0].controllerExitCode).toBe(0);
  });
});
