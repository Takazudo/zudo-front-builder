// scripts/release/reconcile-release-assets.mjs (never --clobber) and the
// finalization-only gate / release-status decision tables (#4690). Every `gh`
// call goes to an injected in-memory fake.
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { finalizeDecision } from "../release/finalize-gate.mjs";
import { plan, reconcile } from "../release/reconcile-release-assets.mjs";
import { assetInventory, expectedReleaseAssets } from "../release/release-assets-contract.mjs";
import {
  distTagsSettled,
  recommendNextAction,
  resolveSource,
  runMatchesTag,
} from "../release/release-status.mjs";
import { alsoLatest, main as alsoLatestMain } from "../release/resolve-also-latest.mjs";

const sha256 = (data) => createHash("sha256").update(Buffer.from(data)).digest("hex");
const dirs = [];
afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});
function localDir(files) {
  const dir = mkdtempSync(join(tmpdir(), "zfb-reconcile-test-"));
  dirs.push(dir);
  for (const [name, data] of Object.entries(files)) writeFileSync(join(dir, name), data);
  return dir;
}

/** In-memory Release: { name: bytes }. Records every call. */
function fakeGh(remote, { digests = true } = {}) {
  const calls = [];
  const gh = (args) => {
    calls.push(args);
    if (args[0] === "release" && args[1] === "view") {
      return JSON.stringify({
        assets: Object.entries(remote).map(([name, bytes]) => ({
          name,
          state: "uploaded",
          digest: digests ? `sha256:${sha256(bytes)}` : null,
        })),
      });
    }
    if (args[0] === "release" && args[1] === "download") {
      const name = args[args.indexOf("--pattern") + 1];
      writeFileSync(join(args[args.indexOf("--dir") + 1], name), remote[name]);
      return "";
    }
    if (args[0] === "release" && args[1] === "upload") return "";
    throw new Error(`unexpected gh ${args.join(" ")}`);
  };
  return { gh, calls };
}
const uploads = (calls) => calls.filter((args) => args[1] === "upload");
const log = () => {};

describe("reconcile-release-assets", () => {
  it("uploads missing, skips identical, never passes --clobber", () => {
    const dir = localDir({ "a.tar.gz": "A", "a.tar.gz.sha256": "sumA" });
    const { gh, calls } = fakeGh({ "a.tar.gz": "A" });
    const result = reconcile({ tag: "v1.0.0", dir, repo: "o/r", gh, log });
    expect(result.ok).toBe(true);
    expect(result.decisions.map((d) => [d.name, d.action])).toEqual([
      ["a.tar.gz", "skip"],
      ["a.tar.gz.sha256", "upload"],
    ]);
    const [upload] = uploads(calls);
    expect(upload).toEqual([
      "release",
      "upload",
      "v1.0.0",
      "--repo",
      "o/r",
      join(dir, "a.tar.gz.sha256"),
    ]);
    expect(calls.flat()).not.toContain("--clobber");
  });

  it("fails on a different remote digest, naming the asset, and uploads nothing at all", () => {
    const dir = localDir({ "a.tar.gz": "A", "b.tar.gz": "B" });
    const { gh, calls } = fakeGh({ "a.tar.gz": "tampered" });
    const result = reconcile({ tag: "v1.0.0", dir, repo: "o/r", gh, log });
    expect(result.ok).toBe(false);
    expect(result.problems[0]).toMatch(/^a\.tar\.gz: remote sha256 .* differs from local/);
    expect(uploads(calls)).toEqual([]);
  });

  it("hashes a downloaded copy when GitHub reports no digest", () => {
    const dir = localDir({ "a.tar.gz": "A" });
    const { gh, calls } = fakeGh({ "a.tar.gz": "A" }, { digests: false });
    const result = reconcile({ tag: "v1.0.0", dir, repo: "o/r", gh, log });
    expect(result.decisions[0]).toMatchObject({ action: "skip", remote: { source: "downloaded" } });
    expect(calls.some((args) => args[1] === "download")).toBe(true);
  });

  it("--verify-only: identical passes; missing or different fails; never uploads", () => {
    const dir = localDir({ "d.node": "D", "e.json": "E" });
    let fake = fakeGh({ "d.node": "D", "e.json": "E" });
    expect(
      reconcile({ tag: "v1.0.0", dir, repo: "o/r", gh: fake.gh, log, verifyOnly: true }).ok,
    ).toBe(true);
    fake = fakeGh({ "d.node": "D" });
    const missing = reconcile({
      tag: "v1.0.0",
      dir,
      repo: "o/r",
      gh: fake.gh,
      log,
      verifyOnly: true,
    });
    expect(missing.problems).toEqual(["e.json: missing from Release v1.0.0"]);
    expect(uploads(fake.calls)).toEqual([]);
  });

  it("treats duplicate or not-uploaded remote assets as conflicts", () => {
    const local = [{ name: "a", sha256: sha256("A") }];
    const digestOf = () => ({ sha256: sha256("A") });
    expect(plan(local, [{ name: "a" }, { name: "a" }], digestOf)[0].action).toBe("conflict");
    expect(plan(local, [{ name: "a", state: "starter" }], digestOf)[0].action).toBe("conflict");
  });
});

// ── finalization-only recovery and release-status decisions ────────────────

const V = "6.1.0";
const allAssets = () => {
  const e = expectedReleaseAssets(V);
  return [...e.ci, ...e.darwin, e.mdWasm].map((name) => ({ name, state: "uploaded" }));
};
const npmSummary = (overrides = {}) => ({
  summary: {
    absent: [],
    "present-verified": ["x"],
    "present-unverified": [],
    uncertain: [],
    ...overrides,
  },
});

describe("expected Release assets", () => {
  it("is 3 CI archive pairs, the 10 Darwin inputs and the MD/WASM manifest", () => {
    const e = expectedReleaseAssets(V);
    expect(e.ci).toHaveLength(6);
    expect(e.darwin).toHaveLength(10);
    expect(e.darwin).toContain(`zfb-${V}-x86_64-apple-darwin.qualification.json`);
    expect(e.mdWasm).toBe(`zfb-md-wasm-${V}-shipped-artifacts.json`);
  });
});

describe("finalizeDecision", () => {
  const admitted = { admitted: true };
  it("clears when every package is present-verified and build assets are complete", () => {
    const inventory = assetInventory(
      V,
      allAssets().filter((a) => !a.name.startsWith("zfb-md-wasm")),
    );
    expect(finalizeDecision({ inventory, npm: npmSummary(), admission: admitted })).toEqual({
      cleared: true,
      reasons: [],
    });
  });

  it.each([
    [
      "a missing build asset (use normal recovery)",
      { assets: allAssets().filter((a) => !a.name.endsWith("windows-msvc.zip")) },
      /build assets missing .*use the normal recovery dispatch/,
    ],
    [
      "an absent package",
      { npm: npmSummary({ absent: ["create-zfb"] }) },
      /not published \(create-zfb\)/,
    ],
    [
      "an unverified package",
      { npm: npmSummary({ "present-unverified": ["a"] }) },
      /present-unverified/,
    ],
    ["an uncertain registry read", { npm: npmSummary({ uncertain: ["a"] }) }, /uncertain/],
    ["a non-admitted Darwin verdict", { admission: { admitted: false } }, /not admitted/],
  ])("blocks on %s", (_label, change, reason) => {
    const decision = finalizeDecision({
      inventory: assetInventory(V, change.assets ?? allAssets()),
      npm: change.npm ?? npmSummary(),
      admission: change.admission ?? admitted,
    });
    expect(decision.cleared).toBe(false);
    expect(decision.reasons.join("\n")).toMatch(reason);
  });
});

describe("recommendNextAction", () => {
  const green = { passed: true, results: [] };
  const state = (overrides = {}) => ({
    tag: { commit: "c".repeat(40) },
    source: { kind: "tag", commit: "c".repeat(40) },
    release: { isDraft: false, targetMismatch: false },
    runs: [],
    assets: assetInventory(V, allAssets()),
    sourceCi: green,
    admission: { admitted: true },
    npm: npmSummary(),
    distTags: { settled: true },
    ...overrides,
  });
  const without = (pred) =>
    assetInventory(
      V,
      allAssets().filter((a) => !pred(a.name)),
    );

  it.each([
    ["complete", {}, "complete"],
    [
      "published Release without a tag ref",
      { tag: {}, source: { kind: null, problem: "tag-missing" } },
      "blocked:tag-missing",
    ],
    [
      "no Release and no tag",
      { tag: {}, source: { kind: null, problem: "release-missing" }, release: null },
      "blocked:release-missing",
    ],
    [
      "draft with a branch-name target and no tag",
      {
        tag: {},
        source: { kind: null, problem: "release-target-not-sha" },
        release: { isDraft: true },
      },
      "blocked:release-target-not-sha",
    ],
    [
      "draft without a tag ref, Darwin inputs missing",
      {
        tag: {},
        source: { kind: "draft-target", commit: "d".repeat(40) },
        release: { isDraft: true },
        assets: without((n) => n.includes("apple-darwin")),
        admission: null,
      },
      "prepare-darwin",
    ],
    [
      "draft without a tag ref, admitted",
      {
        tag: {},
        source: { kind: "draft-target", commit: "d".repeat(40) },
        release: { isDraft: true },
      },
      "publish-draft",
    ],
    ["Release missing", { release: null }, "blocked:release-missing"],
    ["run in progress", { runs: [{ status: "in_progress" }] }, "blocked:release-run-in-progress"],
    [
      "source CI pending",
      { sourceCi: { passed: false, results: [{ state: "pending" }] } },
      "blocked:source-ci-pending",
    ],
    [
      "source CI failed",
      { sourceCi: { passed: false, results: [{ state: "failed" }, { state: "pending" }] } },
      "blocked:source-ci-failed",
    ],
    [
      "source CI missing",
      { sourceCi: { passed: false, results: [{ state: "missing" }] } },
      "blocked:source-ci-missing",
    ],
    [
      "draft without Darwin inputs",
      {
        release: { isDraft: true },
        assets: without((n) => n.includes("apple-darwin")),
        admission: null,
      },
      "prepare-darwin",
    ],
    [
      "draft with rejected Darwin inputs",
      { release: { isDraft: true }, admission: { admitted: false } },
      "prepare-darwin",
    ],
    ["draft, admitted", { release: { isDraft: true } }, "publish-draft"],
    [
      "published without Darwin inputs",
      { assets: without((n) => n.endsWith(".qualification.json")), admission: null },
      "blocked:darwin-inputs-missing-on-published-release",
    ],
    ["npm uncertain", { npm: npmSummary({ uncertain: ["a"] }) }, "blocked:npm-uncertain"],
    [
      "npm unverified",
      { npm: npmSummary({ "present-unverified": ["a"] }) },
      "blocked:npm-unverified",
    ],
    ["a package absent", { npm: npmSummary({ absent: ["a"] }) }, "dispatch-recovery"],
    ["a CI archive missing", { assets: without((n) => n.endsWith(".zip")) }, "dispatch-recovery"],
    [
      "digest asset missing",
      { assets: without((n) => n.startsWith("zfb-md-wasm")) },
      "dispatch-finalize",
    ],
    ["dist-tags unsettled", { distTags: { settled: false } }, "dispatch-finalize"],
  ])("%s → %s", (_label, overrides, action) => {
    expect(recommendNextAction(state(overrides))).toBe(action);
  });
});

describe("distTagsSettled", () => {
  const tags = (latest, next) => ({
    "@takazudo/zfb": { tags: { latest, ...(next ? { next } : {}) } },
  });
  it("stable: latest at/above the version and no stale next", () => {
    expect(distTagsSettled("6.1.0", tags("6.1.0")).settled).toBe(true);
    expect(distTagsSettled("6.1.0", tags("6.2.0")).settled).toBe(true);
    expect(distTagsSettled("6.1.0", tags("6.0.1")).settled).toBe(false);
    expect(distTagsSettled("6.1.0", tags("6.1.0", "6.1.0-next.3")).problems[0]).toContain(
      "stale next",
    );
    expect(distTagsSettled("6.1.0", tags("6.1.0", "7.0.0-next.1")).settled).toBe(true);
  });
  it("prerelease: next at/above the version; latest follows while still prerelease", () => {
    expect(distTagsSettled("7.0.0-next.2", tags("6.1.0", "7.0.0-next.2")).settled).toBe(true);
    expect(distTagsSettled("7.0.0-next.2", tags("6.1.0", "7.0.0-next.1")).settled).toBe(false);
    expect(distTagsSettled("1.0.0-next.2", tags("1.0.0-next.1", "1.0.0-next.2")).settled).toBe(
      false,
    );
  });
});

describe("resolveSource", () => {
  const SHA = "e".repeat(40);
  it.each([
    [
      "tag ref present",
      { commit: SHA, tree: "t" },
      { isDraft: false },
      { kind: "tag", commit: SHA },
    ],
    [
      "tag ref wins over a draft target",
      { commit: SHA },
      { isDraft: true, targetCommitish: "f".repeat(40) },
      { kind: "tag", commit: SHA },
    ],
    [
      "draft with a 40-hex target",
      {},
      { isDraft: true, targetCommitish: SHA },
      { kind: "draft-target", commit: SHA },
    ],
    [
      "draft with a branch target",
      {},
      { isDraft: true, targetCommitish: "main" },
      { kind: null, problem: "release-target-not-sha" },
    ],
    [
      "published Release without a tag ref",
      {},
      { isDraft: false, targetCommitish: SHA },
      { kind: null, problem: "tag-missing" },
    ],
    ["no Release and no tag", {}, null, { kind: null, problem: "release-missing" }],
  ])("%s", (_label, tagInfo, release, want) => {
    expect(resolveSource({ tagInfo, release })).toMatchObject(want);
  });
});

describe("runMatchesTag (exact run-name match)", () => {
  it.each([
    ["Release v6.1.1", "main", true],
    ["Release v6.1.1 (finalize only)", "main", true],
    ["Release v6.1.1 (dry run)", "main", true],
    ["Release v6.1.1 (finalize only) (dry run)", "main", true],
    ["Release v6.1.10", "main", false],
    ["Release v6.1.1-next.1", "main", false],
    ["Release v6x1x1", "main", false],
    ["Some other title", "v6.1.1", true],
  ])("%s on %s → %s", (displayTitle, headBranch, want) => {
    expect(runMatchesTag({ displayTitle, headBranch }, "v6.1.1")).toBe(want);
  });
});

describe("resolve-also-latest (prerelease dual-tag, fail closed)", () => {
  it.each([
    ["latest", "6.0.1", false],
    ["next", "", true],
    ["next", "7.0.0-next.1", true],
    ["next", "6.0.1", false],
  ])("dist-tag %s with latest=%s → %s", (tag, latest, want) => {
    expect(alsoLatest(tag, latest)).toBe(want);
  });

  const run = async (response) => {
    let out = "";
    let err = "";
    const code = await alsoLatestMain(["--dist-tag", "next"], {
      request: async () => (typeof response === "function" ? response() : response),
      out: { write: (c) => (out += c) },
      err: { write: (c) => (err += c) },
    });
    return { code, out, err };
  };
  const body = (obj, status = 200) => ({ status, body: Buffer.from(JSON.stringify(obj)) });

  it("prints 1 while latest is a prerelease, empty once it is stable", async () => {
    expect((await run(body({ latest: "7.0.0-next.1", next: "7.0.0-next.1" }))).out).toBe("1\n");
    expect((await run(body({ latest: "6.0.1" }))).out).toBe("\n");
  });

  it("treats only a definitive 404 (never published) as an empty latest", async () => {
    const result = await run({ status: 404, body: Buffer.from("not found") });
    expect(result).toMatchObject({ code: 0, out: "1\n" });
  });

  it.each([
    ["a 5xx", { status: 503, body: Buffer.from("down") }],
    ["an auth failure", { status: 401, body: Buffer.from("no") }],
    ["malformed JSON", { status: 200, body: Buffer.from("{x") }],
    [
      "a network error",
      () => {
        throw new Error("ECONNRESET");
      },
    ],
  ])("fails closed (exit 2, no decision) on %s", async (_label, response) => {
    const result = await run(response);
    expect(result.code).toBe(2);
    expect(result.out).toBe("");
    expect(result.err).toContain("failing closed");
  });

  it("does not read the registry for a latest publish", async () => {
    let out = "";
    const code = await alsoLatestMain(["--dist-tag", "latest"], {
      request: async () => {
        throw new Error("must not be called");
      },
      out: { write: (c) => (out += c) },
      err: { write: () => {} },
    });
    expect([code, out]).toEqual([0, "\n"]);
  });
});
