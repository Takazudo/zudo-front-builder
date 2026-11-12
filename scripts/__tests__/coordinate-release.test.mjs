// scripts/release/coordinate-release.mjs (#4701). Every GitHub call goes
// through an injected fake that records it, so these tests can show which
// paths write (tag, Release, dispatch) and that a dry run, a stop and an
// owner-acknowledgement block write nothing.
import { describe, expect, it } from "vite-plus/test";
import { PUBLISHED_PACKAGES } from "../retire-next-dist-tag.mjs";
import { GhError } from "../release/gh.mjs";
import {
  CANDIDATE_CONTRACTS,
  CHANGELOG_LANES,
  CoordinatorError,
  DOWNGRADE_MARKER,
  PACKAGE_MANIFESTS,
  SHIPPED_SIZES_PATH,
  bodyDigest,
  bodyHasDowngradeMarker,
  createTag,
  decidePlan,
  dispatchRelease,
  gate,
  isRealRunForTag,
  main,
  publishRelease,
  validateInputs,
} from "../release/coordinate-release.mjs";

const REPO = "Takazudo/zudo-front-builder";
const VERSION = "6.2.0";
const TAG = `v${VERSION}`;
const SHA = "a".repeat(40);
const OTHER = "b".repeat(40);
const MAIN = "c".repeat(40);
const CONTROL = "d".repeat(40);
const RELEASE_ID = 777;

// Any non-GET gh call: an explicit method, or an implicit POST (`gh api` with a
// body via --input/-f/-F/--field/--raw-field and no -X GET).
const WRITE = (args) =>
  (args[0] === "api" &&
    (args.some((a) => ["POST", "PATCH", "PUT", "DELETE"].includes(a)) ||
      (args.some((a) => ["--input", "-f", "-F", "--field", "--raw-field"].includes(a)) &&
        !args.includes("GET")))) ||
  (args[0] === "workflow" && args[1] === "run") ||
  (args[0] === "release" && !["view", "list", "download"].includes(args[1]));

function candidateFiles(version = VERSION) {
  const files = {};
  for (const [name, path] of Object.entries(PACKAGE_MANIFESTS)) {
    files[path] = JSON.stringify({ name, version });
  }
  for (const lane of CHANGELOG_LANES) {
    files[`docs/src/content/docs/changelog/${lane}/v${version}.mdx`] = "---\n---\n";
  }
  files[SHIPPED_SIZES_PATH] = JSON.stringify({ measuredOnVersion: version });
  for (const c of CANDIDATE_CONTRACTS) files[c.path] = `# ${c.marker}\n`;
  return files;
}

function baseStatus(world) {
  return {
    tag: { name: TAG, ...(world.remoteTag ? { commit: world.remoteTag } : {}) },
    source: {
      kind: world.remoteTag ? "tag" : "draft-target",
      commit: world.remoteTag ?? world.release?.target_commitish,
      tree: "e".repeat(40),
    },
    release: world.release
      ? {
          exists: true,
          isDraft: world.release.draft,
          targetCommitish: world.release.target_commitish,
          url: "u",
        }
      : null,
    assets: {
      conflicting: [],
      darwin: { missing: [] },
      ci: { missing: [] },
      mdWasm: { missing: [] },
    },
    sourceCi: { passed: true, results: [] },
    admission: { admitted: true },
    npm: { summary: { absent: [...PUBLISHED_PACKAGES], "present-unverified": [], uncertain: [] } },
    runs: world.runs,
    recommendedAction: world.release?.draft ? "publish-draft" : "dispatch-recovery",
  };
}

function makeWorld(tweak = () => {}) {
  const world = {
    files: candidateFiles(),
    mainHead: MAIN,
    onMain: true,
    remoteTag: null,
    release: {
      id: RELEASE_ID,
      tag_name: TAG,
      draft: true,
      prerelease: false,
      body: `notes\r\n${DOWNGRADE_MARKER}\r\n`,
      target_commitish: SHA,
    },
    latest: "v6.1.0",
    runs: [],
    provenance: [
      {
        name: "@takazudo/zfb-darwin-arm64",
        status: "downgrade",
        version: VERSION,
        earlierAttested: "1.0.0",
      },
      {
        name: "@takazudo/zfb-darwin-x64",
        status: "downgrade",
        version: VERSION,
        earlierAttested: "2.13.0",
      },
    ],
    policy: null,
    tagPost: "ok", // ok | denied | race-same | race-other
    tagReadableAfter: 0,
    dispatchCreatesRun: true,
    statusTweak: (s) => s,
    env: {
      GITHUB_ACTIONS: "true",
      GITHUB_REF: "refs/heads/main",
      GITHUB_SHA: CONTROL,
      GITHUB_WORKFLOW_REF: `${REPO}/.github/workflows/release-coordinator.yml@refs/heads/main`,
      GITHUB_RUN_ID: "99",
    },
    workflowsTrees: { [SHA]: "1".repeat(40), [CONTROL]: "2".repeat(40) },
    calls: [],
    clock: 1_000_000,
  };
  tweak(world);
  return world;
}

function notFound(args) {
  return new GhError(args, { stderr: "gh: Not Found (HTTP 404)" });
}

function makeIo(world) {
  let tagReads = 0;
  const gh = (args, { input } = {}) => {
    world.calls.push({ args, input });
    const path = args[0] === "api" ? args.find((a) => a.startsWith("repos/")) : null;
    if (args[0] === "api" && path === `repos/${REPO}/git/ref/tags/${TAG}`) {
      tagReads++;
      if (!world.remoteTag || tagReads <= world.tagReadableAfter) throw notFound(args);
      return JSON.stringify({ object: { type: "commit", sha: world.remoteTag } });
    }
    if (args[0] === "api" && path === `repos/${REPO}/git/refs`) {
      const body = JSON.parse(input);
      if (world.tagPost === "denied") {
        throw new GhError(args, {
          stderr: "gh: Resource not accessible by integration (HTTP 403)",
          stdout: "HTTP/2.0 403 Forbidden\r\nX-Github-Request-Id: ABCD:1234:EF\r\n\r\n{}",
        });
      }
      if (world.tagPost === "race-same") {
        world.remoteTag = SHA;
        throw new GhError(args, { stderr: "gh: Reference already exists (HTTP 422)" });
      }
      if (world.tagPost === "race-other") {
        world.remoteTag = OTHER;
        throw new GhError(args, { stderr: "gh: Reference already exists (HTTP 422)" });
      }
      world.remoteTag = body.sha;
      return "HTTP/2.0 201 Created\r\n\r\n{}";
    }
    if (args[0] === "api" && path === `repos/${REPO}/releases?per_page=100`) {
      expect(args).toContain("--paginate");
      return world.release ? `${JSON.stringify(world.release)}\n` : "";
    }
    if (args[0] === "api" && path === `repos/${REPO}/releases/${RELEASE_ID}`) {
      if (args.includes("PATCH")) {
        const body = JSON.parse(input);
        world.patch = body;
        world.release = { ...world.release, draft: body.draft, prerelease: body.prerelease };
        if (body.make_latest === "true") world.latest = TAG;
        return "{}";
      }
      return JSON.stringify(world.release);
    }
    if (args[0] === "api" && path === `repos/${REPO}/releases/latest`) {
      return JSON.stringify({ tag_name: world.latest });
    }
    if (args[0] === "run" && args[1] === "list") {
      const opt = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
      const limit = Number(opt("--limit"));
      const page = world.runs.filter(
        (run) =>
          (!opt("--branch") || run.headBranch === opt("--branch")) &&
          (!opt("--event") || run.event === opt("--event")),
      );
      if (world.runListFull)
        return JSON.stringify(Array.from({ length: limit }, () => page[0] ?? {}));
      return JSON.stringify(page.slice(0, limit));
    }
    if (args[0] === "workflow" && args[1] === "run") {
      if (world.dispatchCreatesRun) {
        world.runs.push({
          databaseId: 4242,
          attempt: 1,
          status: "queued",
          conclusion: null,
          event: "workflow_dispatch",
          headBranch: args[args.indexOf("--ref") + 1].replace(/^refs\/tags\//, ""),
          headSha: world.remoteTag,
          displayTitle: `Release ${TAG}`,
          createdAt: new Date(world.clock).toISOString(),
          url: "https://example.test/runs/4242",
        });
      }
      return "";
    }
    throw new Error(`unexpected gh ${args.join(" ")}`);
  };
  const git = (args) => {
    if (args[0] === "cat-file") {
      if (args[2] !== `${SHA}^{commit}` && args[2] !== `${OTHER}^{commit}`)
        throw new Error("missing");
      return "";
    }
    if (args[0] === "rev-parse" && args[1] === "refs/remotes/origin/main^{commit}")
      return `${world.mainHead}\n`;
    if (args[0] === "rev-parse" && args[1].endsWith(":.github/workflows")) {
      const tree = world.workflowsTrees[args[1].split(":")[0]];
      if (!tree) throw new Error("no tree");
      return `${tree}\n`;
    }
    if (args[0] === "merge-base") {
      if (!world.onMain) throw new Error("not ancestor");
      return "";
    }
    if (args[0] === "show") {
      const [, path] = args[1].split(/:(.*)/s);
      if (!(path in world.files)) throw new Error("no such path");
      return world.files[path];
    }
    throw new Error(`unexpected git ${args.join(" ")}`);
  };
  const logs = [];
  return {
    io: {
      repo: REPO,
      repoRoot: "/fake",
      env: world.env,
      gh,
      git,
      collectStatus: async () => world.statusTweak(baseStatus(world)),
      checkProvenance: async () => world.provenance,
      loadPolicy: () => {
        if (world.policy instanceof Error) throw world.policy;
        return world.policy;
      },
      sleep: async () => {
        world.clock += 5000;
      },
      now: () => world.clock,
      log: (line) => logs.push(line),
    },
    logs,
  };
}

const writes = (world) => world.calls.filter((c) => WRITE(c.args));
const inputs = { version: VERSION, sha: SHA };

async function rejectsWith(promise, code) {
  const error = await promise.then(
    () => null,
    (e) => e,
  );
  expect(error).toBeInstanceOf(CoordinatorError);
  expect(error.code).toBe(code);
  return error;
}

describe("inputs and pure helpers", () => {
  it("requires an exact version and a full lowercase SHA (never a branch or short SHA)", () => {
    expect(validateInputs(inputs).tag).toBe(TAG);
    for (const bad of [
      { version: "v6.2.0", sha: SHA },
      { version: "6.2", sha: SHA },
      { version: "6.2.0-alpha.1", sha: SHA },
      { version: VERSION, sha: "main" },
      { version: VERSION, sha: SHA.slice(0, 12) },
      { version: VERSION, sha: SHA.toUpperCase() },
    ]) {
      expect(() => validateInputs(bad)).toThrow(CoordinatorError);
    }
  });

  it("matches the owner's marker only as an exact line, CRLF tolerant", () => {
    expect(bodyHasDowngradeMarker(`a\r\n${DOWNGRADE_MARKER}\r\nb`)).toBe(true);
    expect(bodyHasDowngradeMarker(`a ${DOWNGRADE_MARKER}`)).toBe(false);
    expect(bodyHasDowngradeMarker(`  ${DOWNGRADE_MARKER}`)).toBe(false);
    expect(bodyHasDowngradeMarker("")).toBe(false);
  });

  it("counts real and finalize runs for exactly this tag, never dry runs or look-alikes", () => {
    expect(isRealRunForTag({ displayTitle: `Release ${TAG}` }, TAG)).toBe(true);
    expect(isRealRunForTag({ displayTitle: `Release ${TAG} (finalize only)` }, TAG)).toBe(true);
    expect(isRealRunForTag({ displayTitle: `Release ${TAG} (dry run)` }, TAG)).toBe(false);
    expect(isRealRunForTag({ displayTitle: `Release ${TAG}0` }, TAG)).toBe(false);
    expect(isRealRunForTag({ displayTitle: `Release ${TAG}-next.1` }, TAG)).toBe(false);
  });
});

describe("decidePlan", () => {
  const plan = (tweak) => {
    const world = makeWorld(tweak);
    return decidePlan(world.statusTweak(baseStatus(world)), { sha: SHA, tag: TAG });
  };

  it("starts from a draft at the candidate when Step 0 says publish-draft", () => {
    expect(plan().plan).toBe("start");
  });

  it("resumes when the tag already exists at the candidate", () => {
    expect(plan((w) => (w.remoteTag = SHA)).plan).toBe("resume");
  });

  it("resumes a published Release that never got a release.yml run", () => {
    const p = plan((w) => {
      w.remoteTag = SHA;
      w.release.draft = false;
    });
    expect(p.plan).toBe("resume");
  });

  it("refuses a published, never-run Release when anything is already on npm", () => {
    const p = plan((w) => {
      w.remoteTag = SHA;
      w.release.draft = false;
      w.statusTweak = (s) => ({ ...s, npm: { summary: { absent: PUBLISHED_PACKAGES.slice(1) } } });
    });
    expect(p.plan).toBe("stop");
    expect(p.reason).toMatch(/^PUBLISHED_NOT_RESUMABLE/);
  });

  it("never moves a tag that names another commit", () => {
    const p = plan((w) => (w.remoteTag = OTHER));
    expect(p.plan).toBe("stop");
    expect(p.reason).toMatch(/^TAG_ELSEWHERE/);
  });

  it("never substitutes a draft that targets another commit", () => {
    const p = plan((w) => (w.release.target_commitish = OTHER));
    expect(p.reason).toMatch(/^SOURCE_MISMATCH/);
  });

  it("hands Darwin back to the owner's Mac instead of proceeding", () => {
    const p = plan((w) => (w.statusTweak = (s) => ({ ...s, recommendedAction: "prepare-darwin" })));
    expect(p.plan).toBe("stop");
    expect(p.reason).toMatch(/owner's Mac/);
  });

  it("reuses an active or successful run for this tag and SHA (re-send = no duplicate)", () => {
    const run = {
      databaseId: 1,
      displayTitle: `Release ${TAG}`,
      headSha: SHA,
      event: "workflow_dispatch",
      headBranch: TAG,
    };
    expect(plan((w) => (w.runs = [{ ...run, status: "in_progress" }])).plan).toBe("reuse");
    expect(
      plan((w) => (w.runs = [{ ...run, status: "completed", conclusion: "success" }])).plan,
    ).toBe("reuse");
  });

  it("does not re-run a failed release; it defers to release-status", () => {
    const p = plan(
      (w) =>
        (w.runs = [
          {
            databaseId: 1,
            displayTitle: `Release ${TAG}`,
            headSha: SHA,
            status: "completed",
            conclusion: "failure",
            event: "workflow_dispatch",
            headBranch: TAG,
          },
        ]),
    );
    expect(p.reason).toMatch(/^RUN_FAILED/);
  });

  it("stops on a run for this tag that built another source", () => {
    const p = plan(
      (w) =>
        (w.runs = [
          {
            databaseId: 1,
            displayTitle: `Release ${TAG}`,
            headBranch: TAG,
            headSha: OTHER,
            status: "completed",
            conclusion: "success",
          },
        ]),
    );
    expect(p.reason).toMatch(/^RUN_FOR_OTHER_SOURCE/);
  });

  it("leaves a release that already has a recovery/finalize run from main to Step 0", () => {
    const p = plan(
      (w) =>
        (w.runs = [
          {
            databaseId: 9,
            displayTitle: `Release ${TAG}`,
            headBranch: "main",
            headSha: MAIN,
            event: "workflow_dispatch",
            status: "completed",
            conclusion: "success",
          },
        ]),
    );
    expect(p.reason).toMatch(/^RECOVERY_STARTED/);
  });

  it("ignores dry runs when deciding", () => {
    expect(
      plan(
        (w) =>
          (w.runs = [
            {
              databaseId: 1,
              displayTitle: `Release ${TAG} (dry run)`,
              headSha: SHA,
              status: "completed",
              conclusion: "success",
            },
          ]),
      ).plan,
    ).toBe("start");
  });
});

describe("gate (the dry run): read-only", () => {
  it("passes a complete candidate with the owner's marker and writes nothing", async () => {
    const world = makeWorld();
    const { io } = makeIo(world);
    const result = await gate(io, inputs);
    expect(result.plan).toBe("start");
    expect(result.provenance.acknowledgement).toBe("release-body-marker");
    expect(result.control.sha).toBe(CONTROL);
    expect(writes(world)).toEqual([]);
  });

  it("stops before any write when the downgrade is not acknowledged by the owner", async () => {
    const world = makeWorld((w) => (w.release.body = "notes only"));
    const { io } = makeIo(world);
    const error = await rejectsWith(gate(io, inputs), "OWNER_ACK_REQUIRED");
    expect(error.message).toMatch(/Only the owner may acknowledge/);
    expect(writes(world)).toEqual([]);
  });

  it("accepts the owner's committed policy instead of the marker", async () => {
    const world = makeWorld((w) => {
      w.release.body = "notes only";
      w.policy = {
        schema: "zfb-provenance-policy/v1",
        decisions: ["@takazudo/zfb-darwin-arm64", "@takazudo/zfb-darwin-x64"].map((pkg) => ({
          package: pkg,
          unattested_from_version: "6.1.0",
          reason: "r",
          decided_by: "owner",
          issue: "https://example.test/1",
        })),
      };
    });
    const { io } = makeIo(world);
    expect((await gate(io, inputs)).provenance.acknowledgement).toBe("committed-policy");
  });

  it("fails closed (exit 3) on a registry error or an unreadable policy", async () => {
    const registry = makeWorld(
      (w) =>
        (w.provenance = [
          { name: "@takazudo/zfb-darwin-arm64", status: "error", detail: "HTTP 503" },
        ]),
    );
    const e1 = await rejectsWith(gate(makeIo(registry).io, inputs), "REGISTRY_ERROR");
    expect(e1.exitCode).toBe(3);
    const policy = makeWorld((w) => (w.policy = new Error("malformed")));
    expect((await rejectsWith(gate(makeIo(policy).io, inputs), "POLICY_UNREADABLE")).exitCode).toBe(
      3,
    );
  });

  it("refuses a dispatch that is not from main", async () => {
    const world = makeWorld((w) => (w.env.GITHUB_REF = "refs/heads/feature"));
    await rejectsWith(gate(makeIo(world).io, inputs), "CONTROL_NOT_MAIN");
  });

  it.each([
    [
      "a lockstep package at another version",
      (w) =>
        (w.files[PACKAGE_MANIFESTS["create-zfb"]] = JSON.stringify({
          name: "create-zfb",
          version: "6.1.0",
        })),
      /create-zfb is 6\.1\.0/,
    ],
    [
      "a missing changelog lane",
      (w) => delete w.files[`docs/src/content/docs/changelog/zfb-runtime/v${VERSION}.mdx`],
      /zfb-runtime\/v6\.2\.0\.mdx is missing/,
    ],
    [
      "a missing A4 measurement",
      (w) => (w.files[SHIPPED_SIZES_PATH] = JSON.stringify({ measuredOnVersion: "6.1.0" })),
      /measuredOnVersion is 6\.1\.0/,
    ],
    [
      "a candidate whose release.yml predates the coordinator",
      (w) => (w.files[".github/workflows/release.yml"] = "name: Release\n"),
      /predates the coordinator/,
    ],
    ["a candidate not on main", (w) => (w.onMain = false), /is not on main/],
  ])("refuses %s", async (_name, tweak, pattern) => {
    const world = makeWorld(tweak);
    const error = await rejectsWith(gate(makeIo(world).io, inputs), "CANDIDATE_INVALID");
    expect(error.message).toMatch(pattern);
    expect(writes(world)).toEqual([]);
  });

  it("keeps the frozen SHA when main has advanced, with a notice", async () => {
    const world = makeWorld();
    const { io, logs } = makeIo(world);
    const result = await gate(io, inputs);
    expect(result.candidate).toBe(SHA);
    expect(logs.join("\n")).toMatch(/never substituted/);
  });
});

describe("run history (review S1/S2)", () => {
  const failed = {
    databaseId: 11,
    displayTitle: `Release ${TAG}`,
    headBranch: TAG,
    headSha: SHA,
    event: "workflow_dispatch",
    status: "completed",
    conclusion: "failure",
  };
  const dryRuns = Array.from({ length: 8 }, (_, i) => ({
    databaseId: 100 + i,
    displayTitle: `Release ${TAG} (dry run)`,
    headBranch: "main",
    headSha: MAIN,
    event: "workflow_dispatch",
    status: "completed",
    conclusion: "success",
  }));

  it("sees an older failed run behind many newer dry runs and refuses to start again", async () => {
    // release-status's own listing would be cut to its newest 5 matches here.
    const world = makeWorld((w) => {
      w.remoteTag = SHA;
      w.release.draft = false;
      w.runs = [...dryRuns, failed];
      w.statusTweak = (status) => ({ ...status, runs: dryRuns.slice(0, 5) });
    });
    await rejectsWith(gate(makeIo(world).io, inputs), "RUN_FAILED");
    expect(writes(world)).toEqual([]);
  });

  it("treats a run list that reaches its limit as inconclusive, never as 'never ran'", async () => {
    const world = makeWorld((w) => {
      w.remoteTag = SHA;
      w.release.draft = false;
      w.runListFull = true;
    });
    const error = await rejectsWith(gate(makeIo(world).io, inputs), "RUNS_TRUNCATED");
    expect(error.exitCode).toBe(3);
    expect(writes(world)).toEqual([]);
  });

  it("refuses to dispatch when a run appears between the gate and the dispatch", async () => {
    const world = makeWorld((w) => {
      w.remoteTag = SHA;
      w.release.draft = false;
    });
    const { io } = makeIo(world);
    let listings = 0;
    const gh = io.gh;
    io.gh = (args, opts) => {
      if (args[0] === "run" && args[1] === "list" && ++listings === 3) {
        // The gate made the first two listings; an owner's web-UI publish lands now.
        world.runs.push({
          ...failed,
          databaseId: 12,
          event: "release",
          status: "queued",
          conclusion: null,
        });
      }
      return gh(args, opts);
    };
    await rejectsWith(dispatchRelease(io, inputs, { intervalMs: 0 }), "RUN_APPEARED");
    expect(writes(world)).toEqual([]);
  });
});

describe("Release body digest (review S3)", () => {
  it("logs the CR-stripped digest of the body it gated, matching release.yml's normalization", async () => {
    const world = makeWorld();
    const { io, logs } = makeIo(world);
    const result = await gate(io, inputs);
    expect(result.releaseBodySha256).toBe(bodyDigest(`notes\n${DOWNGRADE_MARKER}`));
    expect(bodyDigest("a\r\nb\r\n\n")).toBe(bodyDigest("a\nb"));
    expect(logs.join("\n")).toContain(result.releaseBodySha256);
  });
});

describe("tag", () => {
  it("creates the tag at the candidate through the REST API only", async () => {
    const world = makeWorld();
    const result = await createTag(makeIo(world).io, inputs, { intervalMs: 0 });
    expect(result.tagAction).toBe("created");
    expect(world.remoteTag).toBe(SHA);
    const posts = writes(world);
    expect(posts).toHaveLength(1);
    expect(JSON.parse(posts[0].input)).toEqual({ ref: `refs/tags/${TAG}`, sha: SHA });
  });

  it("is a no-op on a re-send when the tag already names the candidate", async () => {
    const world = makeWorld((w) => (w.remoteTag = SHA));
    const result = await createTag(makeIo(world).io, inputs);
    expect(result.tagAction).toMatch(/resume/);
    expect(writes(world)).toEqual([]);
  });

  it("refuses a tag that names another commit, without writing", async () => {
    const world = makeWorld((w) => (w.remoteTag = OTHER));
    await rejectsWith(createTag(makeIo(world).io, inputs), "TAG_ELSEWHERE");
    expect(writes(world)).toEqual([]);
    expect(world.remoteTag).toBe(OTHER);
  });

  it("resolves a concurrent create at the same SHA as a resume", async () => {
    const world = makeWorld((w) => (w.tagPost = "race-same"));
    expect((await createTag(makeIo(world).io, inputs)).tagAction).toMatch(/concurrently/);
  });

  it("refuses when a concurrent create lands on another SHA", async () => {
    const world = makeWorld((w) => (w.tagPost = "race-other"));
    await rejectsWith(createTag(makeIo(world).io, inputs), "TAG_ELSEWHERE");
  });

  it("records GitHub's response and the candidate/control/workflow SHAs on a refusal, without naming a cause", async () => {
    const world = makeWorld((w) => (w.tagPost = "denied"));
    const error = await rejectsWith(createTag(makeIo(world).io, inputs), "TAG_CREATE_FAILED");
    const d = error.details.diagnostics;
    expect(d.httpStatus).toBe("403");
    expect(d.requestId).toBe("ABCD:1234:EF");
    expect(d.githubResponse).toMatch(/Resource not accessible by integration/);
    expect(d.candidateSha).toBe(SHA);
    expect(d.controlSha).toBe(CONTROL);
    expect(d.workflowRef).toMatch(/release-coordinator\.yml@refs\/heads\/main/);
    expect(d.workflowsTreeDiffers).toBe(true);
    expect(error.message).toMatch(/cause is NOT inferred/);
    expect(error.message).toMatch(/Do not add a PAT/);
    expect(world.remoteTag).toBe(null);
  });

  it("reports a slow read-back as inconclusive (exit 3), never as failure", async () => {
    const world = makeWorld((w) => (w.tagReadableAfter = 100));
    const error = await rejectsWith(
      createTag(makeIo(world).io, inputs, { readbackAttempts: 2, intervalMs: 0 }),
      "TAG_READBACK_PENDING",
    );
    expect(error.exitCode).toBe(3);
  });
});

describe("publish", () => {
  it("publishes the draft in one PATCH without Latest and never touches the body", async () => {
    const world = makeWorld((w) => (w.remoteTag = SHA));
    const result = await publishRelease(makeIo(world).io, inputs);
    expect(result.releaseAction).toBe("published (not Latest)");
    expect(world.patch).toEqual({ draft: false, prerelease: false, make_latest: "false" });
    expect(writes(world)).toHaveLength(1);
    expect(world.latest).toBe("v6.1.0");
  });

  it("requires the tag first", async () => {
    const world = makeWorld();
    await rejectsWith(publishRelease(makeIo(world).io, inputs), "TAG_NOT_AT_CANDIDATE");
    expect(writes(world)).toEqual([]);
  });

  it("is a no-op for an already published Release", async () => {
    const world = makeWorld((w) => {
      w.remoteTag = SHA;
      w.release.draft = false;
    });
    expect((await publishRelease(makeIo(world).io, inputs)).releaseAction).toBe(
      "already-published",
    );
    expect(writes(world)).toEqual([]);
  });
});

describe("dispatch", () => {
  const published = (w) => {
    w.remoteTag = SHA;
    w.release.draft = false;
  };

  it("dispatches release.yml at the tag with dry_run=false and no other input", async () => {
    const world = makeWorld(published);
    const result = await dispatchRelease(makeIo(world).io, inputs, { intervalMs: 0 });
    expect(result.dispatchAction).toBe("dispatched");
    expect(result.run.databaseId).toBe(4242);
    const [call] = writes(world);
    expect(call.args).toEqual([
      "workflow",
      "run",
      "release.yml",
      "--repo",
      REPO,
      "--ref",
      `refs/tags/${TAG}`,
      "-f",
      "dry_run=false",
    ]);
    expect(call.args.join(" ")).not.toMatch(/allow_provenance_downgrade|release_tag|finalize_only/);
  });

  it("reuses an active run on a re-send instead of dispatching again", async () => {
    const world = makeWorld((w) => {
      published(w);
      w.runs = [
        {
          databaseId: 5,
          displayTitle: `Release ${TAG}`,
          headSha: SHA,
          headBranch: TAG,
          event: "workflow_dispatch",
          status: "in_progress",
        },
      ];
    });
    expect((await dispatchRelease(makeIo(world).io, inputs)).dispatchAction).toBe("reused");
    expect(writes(world)).toEqual([]);
  });

  it("refuses to dispatch before the Release is published", async () => {
    const world = makeWorld((w) => (w.remoteTag = SHA));
    await rejectsWith(dispatchRelease(makeIo(world).io, inputs), "RELEASE_NOT_PUBLISHED");
    expect(writes(world)).toEqual([]);
  });

  it("reports an unlisted run as inconclusive (exit 3)", async () => {
    const world = makeWorld((w) => {
      published(w);
      w.dispatchCreatesRun = false;
    });
    const error = await rejectsWith(
      dispatchRelease(makeIo(world).io, inputs, { readbackAttempts: 2, intervalMs: 0 }),
      "DISPATCH_READBACK_PENDING",
    );
    expect(error.exitCode).toBe(3);
  });
});

describe("end to end and CLI", () => {
  it("tag → publish → dispatch from a draft, then a full re-send changes nothing", async () => {
    const world = makeWorld();
    const { io } = makeIo(world);
    await createTag(io, inputs, { intervalMs: 0 });
    await publishRelease(io, inputs);
    await dispatchRelease(io, inputs, { intervalMs: 0 });
    const before = writes(world).length;
    expect(before).toBe(3);
    await createTag(io, inputs, { intervalMs: 0 });
    await publishRelease(io, inputs);
    expect((await dispatchRelease(io, inputs)).dispatchAction).toBe("reused");
    expect(writes(world).length).toBe(before);
  });

  it("returns 0/1/2 from the CLI and never writes on a gate", async () => {
    const ok = makeWorld();
    expect(
      await main(["gate", "--version", VERSION, "--sha", SHA], { io: makeIo(ok).io, env: {} }),
    ).toBe(0);
    expect(writes(ok)).toEqual([]);
    const blocked = makeWorld((w) => (w.release.body = ""));
    expect(
      await main(["gate", "--version", VERSION, "--sha", SHA], { io: makeIo(blocked).io, env: {} }),
    ).toBe(1);
    expect(await main(["gate", "--version", VERSION], { env: {} })).toBe(2);
    expect(await main(["release", "--version", VERSION, "--sha", SHA], { env: {} })).toBe(2);
  });
});
