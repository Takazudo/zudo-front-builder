// Subprocess half of the Darwin admission tests: the verifier CLI's exit codes
// and build-darwin-local.sh's --check-only refusals. Every child process is
// awaited, so this suite runs in the scripts-subprocess vitest project.
import { execFile } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { assetNames } from "../darwin-artifact-contract.mjs";
import { installFakeGh } from "./helpers/fake-gh.mjs";
import {
  ARM,
  VERSION,
  cleanupTmp,
  createSource,
  fixture,
  scriptsDir,
  tmp,
} from "./helpers/darwin-fixture.mjs";

function runAsync(command, args, env) {
  return new Promise((resolve) => {
    execFile(command, args, { encoding: "utf8", env }, (error, stdout, stderr) => {
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
  const run = (repo, args, env) =>
    runAsync("bash", [join(repo, "scripts", "build-darwin-local.sh"), ...args], env);

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

  describe("without a local tag ref: binds to the DRAFT Release (#4690)", () => {
    let fakeBin;
    beforeAll(() => {
      fakeBin = installFakeGh();
    });
    afterAll(() => rmSync(fakeBin, { recursive: true, force: true }));
    const withRelease = (release) => ({
      ...process.env,
      PATH: `${fakeBin}:${process.env.PATH}`,
      FAKE_GH_RELEASE_JSON: JSON.stringify({ tagName: `v${VERSION}`, ...release }),
    });
    const untagged = async () => {
      const box = await sandbox();
      await box.git("tag", "-d", `v${VERSION}`);
      return box;
    };
    const args = (head, out, flag) => [
      "--source",
      head,
      flag,
      `v${VERSION}`,
      "--out-dir",
      out,
      "--check-only",
    ];

    it.each(["--tag", "--upload"])(
      "accepts %s with a draft targeting exactly --source",
      async (flag) => {
        const { repo, head, out } = await untagged();
        const result = await run(
          repo,
          args(head, out, flag),
          withRelease({ isDraft: true, targetCommitish: head }),
        );
        expect(result.stderr).toBe("");
        expect(result.status).toBe(0);
        expect(result.stdout).toContain(`bound to draft Release v${VERSION} targeting ${head}`);
      },
    );

    it("refuses a draft targeting another commit", async () => {
      const { repo, head, out } = await untagged();
      const result = await run(
        repo,
        args(head, out, "--upload"),
        withRelease({ isDraft: true, targetCommitish: "a".repeat(40) }),
      );
      expect(result.status).not.toBe(0);
      expect(result.stderr).toMatch(new RegExp(`targets a{40}, not ${head}`));
    });

    it("refuses a draft whose target is a branch name", async () => {
      const { repo, head, out } = await untagged();
      const result = await run(
        repo,
        args(head, out, "--upload"),
        withRelease({ isDraft: true, targetCommitish: "main" }),
      );
      expect(result.status).not.toBe(0);
      expect(result.stderr).toMatch(/targets 'main', not a full commit id/);
    });

    it("refuses a published (non-draft) Release when the tag ref is absent", async () => {
      const { repo, head, out } = await untagged();
      const result = await run(
        repo,
        args(head, out, "--tag"),
        withRelease({ isDraft: false, targetCommitish: head }),
      );
      expect(result.status).not.toBe(0);
      expect(result.stderr).toMatch(/is not a draft/);
    });

    it("refuses when neither a tag ref nor a Release exists", async () => {
      const { repo, head, out } = await untagged();
      const result = await run(repo, args(head, out, "--tag"), {
        ...withRelease({}),
        FAKE_GH_RELEASE_MISSING: "1",
      });
      expect(result.status).not.toBe(0);
      expect(result.stderr).toMatch(/not found locally and no GitHub Release/);
    });

    it("still requires an existing tag ref to peel to --source, ignoring any draft", async () => {
      const { repo, git, head, out } = await sandbox();
      const other = await git("commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "other");
      await git("tag", "-f", `v${VERSION}`, other);
      const result = await run(
        repo,
        args(head, out, "--tag"),
        withRelease({ isDraft: true, targetCommitish: head }),
      );
      expect(result.stderr).toMatch(/peels to [0-9a-f]{40}, not/);
    });
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
