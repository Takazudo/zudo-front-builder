// release.yml's admit-darwin wrapper (scripts/release/admit-release-darwin.sh)
// against a PATH-stubbed `gh`, and the publish job's placement step
// (scripts/release/place-darwin-carriers.mjs). The wrapper's child processes
// are awaited, so this suite runs in the scripts-subprocess vitest project.
import { execFile } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { assetNames } from "../darwin-artifact-contract.mjs";
import { place, preparePlacement } from "../release/place-darwin-carriers.mjs";
import { GhError } from "../release/gh.mjs";
import { collectStatus, render } from "../release/release-status.mjs";
import { REQUIRED_CONTEXTS } from "../release/verify-source-ci.mjs";
import { installFakeGh } from "./helpers/fake-gh.mjs";
import {
  ARM,
  VERSION,
  X64,
  cleanupTmp,
  createSource,
  fixture,
  scriptsDir,
  tmp,
} from "./helpers/darwin-fixture.mjs";

const TAG = `v${VERSION}`;
const wrapper = join(scriptsDir, "release", "admit-release-darwin.sh");

function runWrapper(args, env) {
  return new Promise((resolve) => {
    execFile("bash", [wrapper, ...args], { encoding: "utf8", env }, (error, stdout, stderr) => {
      resolve({
        status: error ? (typeof error.code === "number" ? error.code : -1) : 0,
        stdout,
        stderr,
      });
    });
  });
}

let source;
let otherSource;
let fakeBin;
beforeAll(() => {
  source = createSource();
  otherSource = createSource({ label: "a different release source" });
  fakeBin = installFakeGh();
});
afterAll(() => {
  rmSync(source.repo, { recursive: true, force: true });
  rmSync(otherSource.repo, { recursive: true, force: true });
  rmSync(fakeBin, { recursive: true, force: true });
});
afterEach(cleanupTmp);

function admit(assetsDir, { commit = source.commit, tree = source.tree, repo = source.repo } = {}) {
  const out = join(tmp("zfb-admission-out-"), "admission");
  const log = join(tmp("zfb-gh-log-"), "gh.log");
  return runWrapper(
    [
      "--tag",
      TAG,
      "--version",
      VERSION,
      "--source-commit",
      commit,
      "--source-tree",
      tree,
      "--repo-root",
      repo,
      "--out",
      out,
    ],
    {
      ...process.env,
      PATH: `${fakeBin}:${process.env.PATH}`,
      FAKE_GH_ASSETS_DIR: assetsDir,
      FAKE_GH_LOG: log,
    },
  ).then((result) => ({ ...result, out, log }));
}

function releaseWith(dir, extra = {}) {
  // The Release also carries hosted assets; admission must ignore them.
  writeFileSync(join(dir, `zfb-${VERSION}-x86_64-unknown-linux-gnu.tar.gz`), "linux");
  for (const [name, bytes] of Object.entries(extra)) writeFileSync(join(dir, name), bytes);
  return dir;
}

describe("admit-release-darwin.sh", () => {
  it("downloads exactly the ten Darwin inputs and admits them", async () => {
    const result = await admit(releaseWith(fixture(source)));
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    const verdict = JSON.parse(readFileSync(join(result.out, "verdict.json"), "utf8"));
    expect(verdict.admitted).toBe(true);
    expect(verdict.source).toEqual({ commit: source.commit, tree: source.tree });
    const inputs = readFileSync(result.log, "utf8");
    expect(inputs).not.toContain("upload");
    const downloaded = new Set(
      [ARM, X64].flatMap((target) => Object.values(assetNames(VERSION, target))),
    );
    for (const name of downloaded) expect(existsSync(join(result.out, "inputs", name))).toBe(true);
    expect(
      existsSync(join(result.out, "inputs", `zfb-${VERSION}-x86_64-unknown-linux-gnu.tar.gz`)),
    ).toBe(false);
  });

  it("fails before downloading when an input is missing, naming the owner-Mac command", async () => {
    const dir = releaseWith(fixture(source));
    unlinkSync(join(dir, assetNames(VERSION, X64).receipt));
    const result = await admit(dir);
    expect(result.status).toBe(1);
    expect(result.stdout).toContain(`missing Darwin input: ${assetNames(VERSION, X64).receipt}`);
    expect(result.stdout).toContain(
      `scripts/build-darwin-local.sh --source ${source.commit} --tag ${TAG} --upload ${TAG}`,
    );
    expect(result.stdout).toContain("Hosted macOS is not used");
    expect(readFileSync(result.log, "utf8")).not.toContain('"download"');
  });

  it("rejects inputs whose receipts name another source commit", async () => {
    // Receipts were produced for `source`; the release source is `otherSource`.
    // Guard the premise: identical fixture commits would make this test vacuous.
    expect(otherSource.commit).not.toBe(source.commit);
    const result = await admit(releaseWith(fixture(source)), {
      commit: otherSource.commit,
      tree: otherSource.tree,
      repo: otherSource.repo,
    });
    expect(result.status).toBe(1);
    expect(result.stderr).toMatch(/REJECT: .*commit/);
    expect(result.stdout).toContain("rejected by scripts/admit-darwin-artifacts.mjs");
    const verdict = JSON.parse(readFileSync(join(result.out, "verdict.json"), "utf8"));
    expect(verdict.admitted).toBe(false);
  });

  it("rejects a mismatched archive checksum", async () => {
    const dir = releaseWith(fixture(source));
    writeFileSync(join(dir, assetNames(VERSION, ARM).archiveSha256), `${"0".repeat(64)}  x\n`);
    const result = await admit(dir);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("REJECT");
  });

  it("refuses a non-empty output directory (fresh admission dir only)", async () => {
    const out = tmp("zfb-admission-dirty-");
    writeFileSync(join(out, "stale"), "x");
    const result = await runWrapper(
      [
        "--tag",
        TAG,
        "--version",
        VERSION,
        "--source-commit",
        source.commit,
        "--source-tree",
        source.tree,
        "--repo-root",
        source.repo,
        "--out",
        out,
      ],
      { ...process.env, PATH: `${fakeBin}:${process.env.PATH}`, FAKE_GH_ASSETS_DIR: out },
    );
    expect(result.status).toBe(2);
  });

  it("rejects a tag that is not v<version>", async () => {
    const result = await runWrapper(
      [
        "--tag",
        "v0.0.1",
        "--version",
        VERSION,
        "--source-commit",
        source.commit,
        "--source-tree",
        source.tree,
        "--repo-root",
        source.repo,
        "--out",
        join(tmp("zfb-x-"), "o"),
      ],
      { ...process.env, PATH: `${fakeBin}:${process.env.PATH}` },
    );
    expect(result.status).toBe(2);
  });
});

describe("place-darwin-carriers", () => {
  async function admitted() {
    const result = await admit(releaseWith(fixture(source)));
    expect(result.status).toBe(0);
    return {
      verdict: JSON.parse(readFileSync(join(result.out, "verdict.json"), "utf8")),
      inputs: join(result.out, "inputs"),
    };
  }

  function packagesRoot() {
    const root = tmp("zfb-place-root-");
    for (const dir of ["packages/zfb-darwin-arm64", "packages/zfb-darwin-x64"]) {
      mkdirSync(join(root, dir), { recursive: true });
      writeFileSync(join(root, dir, "package.json"), "{}");
    }
    return root;
  }

  it("places the admitted binary (0755) and addon for both targets", async () => {
    const { verdict, inputs } = await admitted();
    const root = packagesRoot();
    place({ verdict, inputsDir: inputs, packagesRoot: root });
    for (const [target, dir] of [
      [ARM, "packages/zfb-darwin-arm64"],
      [X64, "packages/zfb-darwin-x64"],
    ]) {
      const binary = join(root, dir, "zfb");
      expect(statSync(binary).mode & 0o777).toBe(0o755);
      expect(readFileSync(join(root, dir, "zfb-css.node"))).toEqual(
        readFileSync(join(inputs, assetNames(VERSION, target).addon)),
      );
    }
  });

  it("re-verifies digests and places nothing when an input changed after admission", async () => {
    const { verdict, inputs } = await admitted();
    const root = packagesRoot();
    writeFileSync(join(inputs, assetNames(VERSION, X64).addon), "swapped after admission");
    expect(() => place({ verdict, inputsDir: inputs, packagesRoot: root })).toThrow(
      /differs from the admitted digest/,
    );
    expect(existsSync(join(root, "packages/zfb-darwin-arm64/zfb"))).toBe(false);
  });

  it("refuses a verdict that was not admitted", async () => {
    const { verdict, inputs } = await admitted();
    expect(() =>
      preparePlacement({ verdict: { ...verdict, admitted: false }, inputsDir: inputs }),
    ).toThrow(/not admitted/);
  });

  it("refuses an archive swapped for another admitted-looking one", async () => {
    const { verdict, inputs } = await admitted();
    const arm = assetNames(VERSION, ARM).archive;
    const x64 = assetNames(VERSION, X64).archive;
    copyFileSync(join(inputs, x64), join(inputs, arm));
    expect(() => preparePlacement({ verdict, inputsDir: inputs })).toThrow(
      new RegExp(`${ARM}: ${arm.replace(/\./g, "\\.")} differs`),
    );
  });
});

describe("release-status (read-only, injected gh/registry)", () => {
  function world(assetsDir, { isDraft = true, tagRef = true, target = "main" } = {}) {
    const calls = [];
    const gh = (args) => {
      calls.push(args);
      const joined = args.join(" ");
      if (joined.includes(`git/ref/tags/${TAG}`)) {
        if (!tagRef) throw new GhError(args, { stderr: "gh: Not Found (HTTP 404)" });
        return JSON.stringify({ object: { type: "commit", sha: source.commit } });
      }
      if (joined.includes(`git/commits/${source.commit}`)) {
        return JSON.stringify({ tree: { sha: source.tree } });
      }
      if (joined.includes("/check-runs")) {
        return REQUIRED_CONTEXTS.filter((c) => !c.prOnly)
          .map((c, i) =>
            JSON.stringify({
              id: i + 1,
              name: c.context,
              status: "completed",
              conclusion: "success",
              app: "github-actions",
              // One check suite per context, resolved below to its workflow file.
              suite: 5000 + i,
            }),
          )
          .join("\n");
      }
      const suite = /check_suite_id=(\d+)/.exec(joined);
      if (suite) {
        const spec = REQUIRED_CONTEXTS.filter((c) => !c.prOnly)[Number(suite[1]) - 5000];
        return JSON.stringify({
          workflow_runs: [
            { path: `.github/workflows/${spec.workflow}`, head_sha: source.commit, event: "push" },
          ],
        });
      }
      if (joined.includes(`commits/${source.commit}/status`))
        return JSON.stringify({ statuses: [] });
      if (args[0] === "release" && args[1] === "view") {
        const assets = readdirSync(assetsDir).map((name) => ({ name, state: "uploaded" }));
        return JSON.stringify({ tagName: TAG, isDraft, targetCommitish: target, url: "u", assets });
      }
      if (args[0] === "release" && args[1] === "download") {
        const out = args[args.indexOf("--dir") + 1];
        for (let i = 0; i < args.length; i++) {
          if (args[i] === "--pattern")
            copyFileSync(join(assetsDir, args[i + 1]), join(out, args[i + 1]));
        }
        return "";
      }
      if (args[0] === "run" && args[1] === "list") return "[]";
      throw new Error(`unexpected gh ${joined}`);
    };
    const request = async () => ({ status: 404, body: Buffer.from("not found") });
    return { gh, calls, request };
  }

  it("draft with admitted Darwin inputs → publish-draft; nothing mutating is called", async () => {
    const { gh, calls, request } = world(fixture(source));
    const status = await collectStatus({
      tag: TAG,
      repo: "o/r",
      gh,
      request,
      repoRoot: source.repo,
      absentReads: 1,
      delayMs: 0,
    });
    // On failure, name the cause: admitFromRelease turns any I/O or git error
    // into {admitted: null, errors}, which would otherwise surface only as a
    // bare true/null mismatch (one unreproduced failure, #4690).
    const why = JSON.stringify({
      admission: status.admission?.errors,
      sourceCi: status.sourceCi?.blocking ?? status.sourceCiError,
      npm: status.npm?.summary,
      action: status.recommendedAction,
    });
    expect(status.admission?.admitted, why).toBe(true);
    expect(status.sourceCi?.passed, why).toBe(true);
    expect(status.npm.summary.absent, why).toHaveLength(11);
    expect(status.recommendedAction, why).toBe("publish-draft");
    expect(render(status)).toContain("recommended next action: publish-draft");
    // #4701: both A12 routes are named, dry run first, pinned to the frozen source.
    expect(render(status)).toContain(
      `gh workflow run release-coordinator.yml --ref main -f version=${status.version} -f sha=${status.source.commit} -f dry_run=true`,
    );
    const verbs = calls.map((args) => `${args[0]} ${args[1]}`);
    expect(verbs.filter((v) => /upload|delete|edit|create|workflow run/.test(v))).toEqual([]);
  });

  it("published release with an accepted-but-invisible package → wait-npm-processing, not recovery", async () => {
    const SECRET = "npm_NEVERPRINTTHIS0123456789";
    const processingName = "@takazudo/zfb-linux-arm64-gnu";
    const base = world(fixture(source), { isDraft: false });
    const gh = (args) => {
      if (args[0] === "run" && args[1] === "list")
        return JSON.stringify([
          {
            databaseId: 99,
            attempt: 1,
            status: "completed",
            conclusion: "failure",
            event: "release",
            headBranch: TAG,
            displayTitle: `Release ${TAG}`,
            createdAt: "2026-11-13T04:00:00Z",
            startedAt: "2026-11-13T04:00:05Z",
            url: "u",
          },
        ]);
      if (args[0] === "api" && args[1].includes("/actions/runs/99/artifacts"))
        return JSON.stringify({
          artifacts: [{ name: "npm-publish-receipts", created_at: "2026-11-13T04:10:00Z" }],
        });
      if (args[0] === "run" && args[1] === "download") {
        writeFileSync(
          join(args[args.indexOf("--dir") + 1], "receipts.tsv"),
          `${processingName}\t${VERSION}\tattested\tpublish-accepted\n`,
        );
        return "";
      }
      return base.gh(args);
    };
    const status = await collectStatus({
      tag: TAG,
      repo: "o/r",
      gh,
      request: base.request,
      repoRoot: source.repo,
      absentReads: 1,
      delayMs: 0,
      npmToken: SECRET,
    });
    expect(status.npm.summary.processing).toEqual([processingName]);
    expect(status.npm.summary.absent).toHaveLength(10);
    expect(status.recommendedAction).toBe("wait-npm-processing");
    const text = render(status);
    expect(text).toContain("recommended next action: wait-npm-processing");
    expect(text).toContain("contact npm support after 24 h");
    expect(text).toContain("2026-11-14T04:10:00.000Z");
    expect(text + JSON.stringify(status)).not.toContain(SECRET);
  });

  it("draft missing a Darwin input → prepare-darwin", async () => {
    const dir = fixture(source);
    unlinkSync(join(dir, assetNames(VERSION, ARM).addon));
    const { gh, request } = world(dir);
    const status = await collectStatus({
      tag: TAG,
      repo: "o/r",
      gh,
      request,
      repoRoot: source.repo,
      absentReads: 1,
      delayMs: 0,
    });
    expect(status.admission).toBeNull();
    expect(status.recommendedAction).toBe("prepare-darwin");
  });

  it("fresh draft without a tag ref binds to its 40-hex target → publish-draft", async () => {
    const { gh, request } = world(fixture(source), { tagRef: false, target: source.commit });
    const status = await collectStatus({
      tag: TAG,
      repo: "o/r",
      gh,
      request,
      repoRoot: source.repo,
      absentReads: 1,
      delayMs: 0,
    });
    expect(status.source).toEqual({
      kind: "draft-target",
      commit: source.commit,
      tree: source.tree,
    });
    expect(status.sourceCi.passed).toBe(true);
    expect(status.admission.admitted).toBe(true);
    expect(status.recommendedAction).toBe("publish-draft");
    expect(render(status)).toContain(`source draft-target commit ${source.commit}`);
  });

  it("fresh draft without a tag ref and without Darwin inputs → prepare-darwin", async () => {
    const dir = fixture(source);
    unlinkSync(join(dir, assetNames(VERSION, X64).receipt));
    const { gh, request } = world(dir, { tagRef: false, target: source.commit });
    const status = await collectStatus({
      tag: TAG,
      repo: "o/r",
      gh,
      request,
      repoRoot: source.repo,
      absentReads: 1,
      delayMs: 0,
    });
    expect(status.recommendedAction).toBe("prepare-darwin");
  });

  it("fresh draft without a tag ref targeting a branch → blocked:release-target-not-sha", async () => {
    const { gh, request } = world(fixture(source), { tagRef: false, target: "main" });
    const status = await collectStatus({
      tag: TAG,
      repo: "o/r",
      gh,
      request,
      repoRoot: source.repo,
      absentReads: 1,
      delayMs: 0,
    });
    expect(status.recommendedAction).toBe("blocked:release-target-not-sha");
    expect(status.npm).toBeNull();
  });

  it("published, admitted, nothing on npm → dispatch-recovery", async () => {
    const { gh, request } = world(fixture(source), { isDraft: false });
    const status = await collectStatus({
      tag: TAG,
      repo: "o/r",
      gh,
      request,
      repoRoot: source.repo,
      absentReads: 1,
      delayMs: 0,
    });
    expect(status.recommendedAction).toBe("dispatch-recovery");
  });
});
