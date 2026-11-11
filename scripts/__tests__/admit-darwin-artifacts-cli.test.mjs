// Subprocess half of the Darwin admission tests: the verifier CLI's exit codes
// and build-darwin-local.sh's --check-only refusals. Every child process is
// awaited, so this suite runs in the scripts-subprocess vitest project.
import { execFile } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { assetNames } from "../darwin-artifact-contract.mjs";
import {
  ARM,
  VERSION,
  cleanupTmp,
  createSource,
  fixture,
  scriptsDir,
  tmp,
} from "./helpers/darwin-fixture.mjs";

function runAsync(command, args) {
  return new Promise((resolve) => {
    execFile(command, args, { encoding: "utf8" }, (error, stdout, stderr) => {
      resolve({
        status: error ? (typeof error.code === "number" ? error.code : -1) : 0,
        stdout,
        stderr,
      });
    });
  });
}

let source;
beforeAll(() => {
  source = createSource();
});
afterAll(() => rmSync(source.repo, { recursive: true, force: true }));
afterEach(cleanupTmp);

describe("admit-darwin-artifacts CLI", () => {
  const cli = join(scriptsDir, "admit-darwin-artifacts.mjs");
  const run = (args) => runAsync(process.execPath, [cli, ...args]);
  const base = (dir) => [
    "--dir",
    dir,
    "--version",
    VERSION,
    "--source-commit",
    source.commit,
    "--source-tree",
    source.tree,
    "--repo-root",
    source.repo,
  ];

  it("exits 0 with a verdict file when admitted", async () => {
    const dir = fixture(source);
    const out = join(tmp("zfb-admit-out-"), "verdict.json");
    const result = await run([...base(dir), "--json-out", out]);
    expect(result.status).toBe(0);
    expect(JSON.parse(readFileSync(out, "utf8")).admitted).toBe(true);
  });

  it("exits 1 when rejected and 2 on usage errors", async () => {
    const dir = fixture(source);
    rmSync(join(dir, assetNames(VERSION, ARM).archive));
    expect((await run(base(dir))).status).toBe(1);
    expect((await run(["--dir", dir])).status).toBe(2);
    expect(
      (await run([...base(dir).slice(0, -2), "--repo-root", tmp("zfb-admit-nogit-")])).status,
    ).toBe(2);
  });
});

describe("build-darwin-local.sh refusals (--check-only, no build)", () => {
  async function sandbox() {
    const repo = tmp("zfb-build-darwin-");
    mkdirSync(join(repo, "scripts"));
    mkdirSync(join(repo, "packages", "zfb"), { recursive: true });
    copyFileSync(
      join(scriptsDir, "build-darwin-local.sh"),
      join(repo, "scripts", "build-darwin-local.sh"),
    );
    writeFileSync(
      join(repo, "packages", "zfb", "package.json"),
      JSON.stringify({ version: VERSION }),
    );
    const git = async (...args) =>
      (
        await runAsync("git", [
          "-C",
          repo,
          "-c",
          "user.email=t@example.invalid",
          "-c",
          "user.name=t",
          ...args,
        ])
      ).stdout.trim();
    await git("init", "-q");
    await git("add", ".");
    await git("commit", "-q", "-m", "release");
    await git("tag", `v${VERSION}`);
    return {
      repo,
      git,
      head: await git("rev-parse", "HEAD"),
      out: join(tmp("zfb-build-out-"), "assets"),
    };
  }
  const run = (repo, args) =>
    runAsync("bash", [join(repo, "scripts", "build-darwin-local.sh"), ...args]);

  it("accepts a clean tree at --source and its tag", async () => {
    const { repo, head, out } = await sandbox();
    const result = await run(repo, [
      "--source",
      head,
      "--tag",
      `v${VERSION}`,
      "--out-dir",
      out,
      "--check-only",
    ]);
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    expect(result.stdout).toContain("source binding OK");
  });

  it("refuses a missing or mismatched --source", async () => {
    const { repo, out } = await sandbox();
    expect((await run(repo, ["--check-only"])).stderr).toMatch(/--source <sha> is required/);
    expect(
      (await run(repo, ["--source", "a".repeat(40), "--out-dir", out, "--check-only"])).stderr,
    ).toMatch(/!= HEAD/);
  });

  it("refuses a tag that is not v<version>, conflicts with --upload, or peels elsewhere", async () => {
    const { repo, git, head, out } = await sandbox();
    expect(
      (await run(repo, ["--source", head, "--tag", "v0.0.1", "--out-dir", out, "--check-only"]))
        .stderr,
    ).toMatch(/--tag v0\.0\.1 != v9\.8\.7/);
    expect(
      (
        await run(repo, [
          "--source",
          head,
          "--tag",
          `v${VERSION}`,
          "--upload",
          "v1.0.0",
          "--check-only",
        ])
      ).stderr,
    ).toMatch(/conflicts with --tag/);
    const other = await git("commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "other");
    await git("tag", "-f", `v${VERSION}`, other);
    expect(
      (
        await run(repo, [
          "--source",
          head,
          "--tag",
          `v${VERSION}`,
          "--out-dir",
          out,
          "--check-only",
        ])
      ).stderr,
    ).toMatch(/peels to [0-9a-f]{40}, not/);
  });

  it("refuses a dirty tree and an out dir inside the checkout", async () => {
    const { repo, head, out } = await sandbox();
    expect(
      (await run(repo, ["--source", head, "--out-dir", join(repo, "inside"), "--check-only"]))
        .stderr,
    ).toMatch(/inside the source checkout/);
    writeFileSync(join(repo, "untracked.txt"), "x");
    expect((await run(repo, ["--source", head, "--out-dir", out, "--check-only"])).stderr).toMatch(
      /working tree is dirty/,
    );
  });
});
