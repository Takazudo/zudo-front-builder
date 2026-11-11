// release.yml release-context bodies: the trigger resolver and the source
// verification (tag peel = checkout, version, published Release, target
// commitish, source tree / control SHA outputs). `gh` is a PATH stub; every
// child process is awaited, so this suite runs in scripts-subprocess.
import { execFile, execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installFakeGh } from "./helpers/fake-gh.mjs";
import { scriptsDir } from "./helpers/darwin-fixture.mjs";

const resolver = join(scriptsDir, "release", "resolve-release-context.sh");
const verifier = join(scriptsDir, "release", "verify-release-source.sh");
const SHA = "a".repeat(40);

function run(script, { env = {}, cwd } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "zfb-ctx-out-"));
  const output = join(dir, "output");
  const summary = join(dir, "summary");
  writeFileSync(output, "");
  return new Promise((resolve) => {
    execFile(
      "bash",
      [script],
      {
        cwd,
        encoding: "utf8",
        env: { ...process.env, GITHUB_OUTPUT: output, GITHUB_STEP_SUMMARY: summary, ...env },
      },
      (error, stdout, stderr) => {
        const outputs = Object.fromEntries(
          readFileSync(output, "utf8")
            .split("\n")
            .filter(Boolean)
            .map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
        );
        rmSync(dir, { recursive: true, force: true });
        resolve({
          status: error ? (typeof error.code === "number" ? error.code : -1) : 0,
          stdout,
          stderr,
          outputs,
        });
      },
    );
  });
}

describe("resolve-release-context.sh", () => {
  const base = { TRIGGER_SHA: SHA, DRY_RUN: "false", FINALIZE_ONLY: "false" };

  it.each([
    [
      "release event",
      { EVENT_NAME: "release", EVENT_RELEASE_TAG: "v6.1.0", TRIGGER_REF: "refs/tags/v6.1.0" },
      { release_tag: "v6.1.0", checkout_ref: "v6.1.0", is_recovery: "false", is_finalize: "false" },
    ],
    [
      "recovery from main",
      { EVENT_NAME: "workflow_dispatch", REQUESTED_TAG: "v6.1.0", TRIGGER_REF: "refs/heads/main" },
      { release_tag: "v6.1.0", is_recovery: "true", is_finalize: "false" },
    ],
    [
      "finalize-only from main",
      {
        EVENT_NAME: "workflow_dispatch",
        REQUESTED_TAG: "v6.1.0",
        TRIGGER_REF: "refs/heads/main",
        FINALIZE_ONLY: "true",
      },
      { release_tag: "v6.1.0", is_recovery: "true", is_finalize: "true" },
    ],
    [
      "tag-less branch dry run",
      { EVENT_NAME: "workflow_dispatch", TRIGGER_REF: "refs/heads/topic", DRY_RUN: "true" },
      { release_tag: "", checkout_ref: SHA, is_recovery: "false", is_finalize: "false" },
    ],
  ])("resolves a %s", async (_label, env, want) => {
    const result = await run(resolver, { env: { ...base, ...env } });
    expect(result.status).toBe(0);
    expect(result.outputs).toMatchObject(want);
  });

  it.each([
    [
      "recovery from a branch other than main",
      { EVENT_NAME: "workflow_dispatch", REQUESTED_TAG: "v6.1.0", TRIGGER_REF: "refs/heads/x" },
      /must run the reviewed workflow from refs\/heads\/main/,
    ],
    [
      "finalize without a release tag",
      {
        EVENT_NAME: "workflow_dispatch",
        TRIGGER_REF: "refs/heads/main",
        FINALIZE_ONLY: "true",
        DRY_RUN: "true",
      },
      /finalize_only=true requires/,
    ],
    [
      "a real manual run without a tag",
      { EVENT_NAME: "workflow_dispatch", TRIGGER_REF: "refs/heads/main" },
      /requires release_tag/,
    ],
    [
      "a malformed tag",
      { EVENT_NAME: "release", EVENT_RELEASE_TAG: "v6.1", TRIGGER_REF: "refs/tags/v6.1" },
      /not an allowed v\* semver tag/,
    ],
  ])("refuses %s", async (_label, env, message) => {
    const result = await run(resolver, { env: { ...base, ...env } });
    expect(result.status).toBe(1);
    expect(result.stdout).toMatch(message);
  });
});

describe("verify-release-source.sh", () => {
  let repo;
  let fakeBin;
  let tagCommit;
  let headCommit;
  const git = (...args) =>
    execFileSync(
      "git",
      ["-C", repo, "-c", "user.email=t@example.invalid", "-c", "user.name=t", ...args],
      { encoding: "utf8" },
    ).trim();

  beforeAll(() => {
    fakeBin = installFakeGh();
    repo = mkdtempSync(join(tmpdir(), "zfb-ctx-repo-"));
    mkdirSync(join(repo, "packages/zfb"), { recursive: true });
    writeFileSync(join(repo, "packages/zfb/package.json"), JSON.stringify({ version: "6.1.0" }));
    git("init", "-q");
    git("add", ".");
    git("commit", "-q", "-m", "release 6.1.0");
    git("tag", "-a", "v6.1.0", "-m", "v6.1.0");
    tagCommit = git("rev-parse", "HEAD");
    writeFileSync(join(repo, "later"), "x");
    git("add", ".");
    git("commit", "-q", "-m", "later");
    headCommit = git("rev-parse", "HEAD");
  });
  afterAll(() => {
    rmSync(repo, { recursive: true, force: true });
    rmSync(fakeBin, { recursive: true, force: true });
  });

  const release = (overrides = {}) =>
    JSON.stringify({
      tagName: "v6.1.0",
      isDraft: false,
      publishedAt: "2026-11-12T00:00:00Z",
      targetCommitish: "main",
      url: "https://example.invalid/r",
      ...overrides,
    });
  const verify = (env) =>
    run(verifier, {
      cwd: repo,
      env: {
        PATH: `${fakeBin}:${process.env.PATH}`,
        CONTROL_SHA: "c".repeat(40),
        WORKFLOW_REF: "Takazudo/zudo-front-builder/.github/workflows/release.yml@refs/heads/main",
        FAKE_GH_RELEASE_JSON: release(),
        ...env,
      },
    });

  it("records source commit, source tree, control SHA and workflow ref", async () => {
    git("checkout", "-q", "v6.1.0");
    const result = await verify({ RELEASE_TAG: "v6.1.0" });
    expect(result.status).toBe(0);
    expect(result.outputs).toMatchObject({
      checkout_sha: tagCommit,
      source_commit: tagCommit,
      source_tree: git("rev-parse", `${tagCommit}^{tree}`),
      release_version: "6.1.0",
      control_sha: "c".repeat(40),
      workflow_ref: "Takazudo/zudo-front-builder/.github/workflows/release.yml@refs/heads/main",
    });
  });

  it("fails when the tag does not peel to the checked-out commit", async () => {
    git("checkout", "-q", headCommit);
    const result = await verify({ RELEASE_TAG: "v6.1.0" });
    expect(result.status).toBe(1);
    expect(result.stdout).toContain(`Checked-out commit ${headCommit} does not match v6.1.0`);
    expect(result.outputs.source_commit).toBeUndefined();
  });

  it.each([
    ["a draft Release", { isDraft: true }, /published, non-draft/],
    ["an unpublished Release", { publishedAt: null }, /published, non-draft/],
    [
      "a target commitish that is another commit",
      { targetCommitish: "HEADSHA" },
      /does not match tag/,
    ],
  ])("fails for %s", async (_label, overrides, message) => {
    git("checkout", "-q", "v6.1.0");
    const target =
      overrides.targetCommitish === "HEADSHA" ? { targetCommitish: headCommit } : overrides;
    const result = await verify({ RELEASE_TAG: "v6.1.0", FAKE_GH_RELEASE_JSON: release(target) });
    expect(result.status).toBe(1);
    expect(result.stdout).toMatch(message);
  });

  it("fails when no Release exists", async () => {
    git("checkout", "-q", "v6.1.0");
    const result = await verify({ RELEASE_TAG: "v6.1.0", FAKE_GH_RELEASE_MISSING: "1" });
    expect(result.status).toBe(1);
    expect(result.stdout).toContain("No GitHub Release exists for v6.1.0");
  });

  it("accepts a tag-less branch dry run without a Release", async () => {
    git("checkout", "-q", headCommit);
    const result = await verify({ RELEASE_TAG: "", FAKE_GH_RELEASE_MISSING: "1" });
    expect(result.status).toBe(0);
    expect(result.outputs).toMatchObject({ source_commit: headCommit, release_version: "" });
  });
});
