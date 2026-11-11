// Tests for scripts/retire-next-dist-tag.mjs.
//
// Three layers:
//   1. compareSemver — the semver 2.0.0 precedence rules the policy rides on.
//      Getting `next.10` vs `next.9` backwards here would delete a live soak tag.
//   2. decideNextTagAction — the policy itself, as a pure function of dist-tags.
//   3. retireNextDistTag — orchestration over an injected fake `npm`, proving the
//      retry path and that a registry failure never rejects (a release must not
//      go red over a leftover dist-tag).
//
// Plus a drift guard: PUBLISHED_PACKAGES must match both the workspace's set of
// publishable packages and scripts/advance-latest-dist-tag.sh's list.

import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

import {
  PUBLISHED_PACKAGES,
  compareSemver,
  decideNextTagAction,
  isTerminalNpmError,
  parseSemver,
  preflightRefusalLines,
  retireNextDistTag,
} from "../retire-next-dist-tag.mjs";

const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

describe("parseSemver", () => {
  it("splits a prerelease version into its precedence-relevant parts", () => {
    expect(parseSemver("1.1.0-next.1")).toEqual({
      major: 1,
      minor: 1,
      patch: 0,
      prerelease: ["next", "1"],
    });
  });

  it("drops build metadata, which semver ignores for precedence", () => {
    expect(parseSemver("2.7.1+build.99").prerelease).toEqual([]);
  });

  it("returns null for a non-semver string", () => {
    expect(parseSemver("latest")).toBeNull();
    expect(parseSemver("")).toBeNull();
    expect(parseSemver(undefined)).toBeNull();
  });
});

describe("compareSemver", () => {
  it("orders by major, then minor, then patch", () => {
    expect(compareSemver("2.0.0", "1.9.9")).toBe(1);
    expect(compareSemver("1.2.0", "1.1.9")).toBe(1);
    expect(compareSemver("1.1.2", "1.1.3")).toBe(-1);
    expect(compareSemver("2.7.1", "2.7.1")).toBe(0);
  });

  it("ranks a prerelease below the stable of the same version", () => {
    expect(compareSemver("1.1.0-next.1", "1.1.0")).toBe(-1);
    expect(compareSemver("1.1.0", "1.1.0-next.1")).toBe(1);
  });

  it("compares numeric prerelease identifiers numerically, not as strings", () => {
    // The whole reason this is hand-rolled: "next.10" < "next.9" lexically.
    expect(compareSemver("0.1.0-next.10", "0.1.0-next.9")).toBe(1);
    expect(compareSemver("0.1.0-next.81", "0.1.0-next.100")).toBe(-1);
  });

  it("ranks numeric identifiers below alphanumeric ones", () => {
    expect(compareSemver("1.0.0-1", "1.0.0-alpha")).toBe(-1);
  });

  it("ranks a longer prerelease above its prefix", () => {
    expect(compareSemver("1.0.0-next.1.2", "1.0.0-next.1")).toBe(1);
  });

  it("throws rather than guessing on an unparseable version", () => {
    expect(() => compareSemver("not-a-version", "1.0.0")).toThrow(/unparseable/);
  });
});

describe("decideNextTagAction", () => {
  it("retires the exact stale state this script was written for", () => {
    const decision = decideNextTagAction({ next: "1.1.0-next.1", latest: "2.7.1" });
    expect(decision.action).toBe("retire");
  });

  it("retires a 'next' that merely equals 'latest'", () => {
    expect(decideNextTagAction({ next: "2.7.1", latest: "2.7.1" }).action).toBe("retire");
  });

  it("keeps a 'next' that is genuinely ahead — an active soak survives a stable patch", () => {
    const decision = decideNextTagAction({ next: "3.0.0-next.1", latest: "2.7.2" });
    expect(decision.action).toBe("keep");
    expect(decision.reason).toMatch(/active soak/);
  });

  it("keeps 'next' while no stable has ever shipped", () => {
    expect(decideNextTagAction({ next: "0.1.0-next.5" }).action).toBe("keep");
  });

  it("reports 'absent' when there is no 'next' tag to retire", () => {
    expect(decideNextTagAction({ latest: "2.7.1" }).action).toBe("absent");
    expect(decideNextTagAction({}).action).toBe("absent");
  });

  it("refuses to delete on a guess when a version will not parse", () => {
    const decision = decideNextTagAction({ next: "garbage", latest: "2.7.1" });
    expect(decision.action).toBe("keep");
    expect(decision.reason).toMatch(/cannot compare/);
  });
});

// Fake `npm` runner: answers `view ... dist-tags --json` from a mutable table
// and records every `dist-tag rm` it is asked to perform. A successful rm
// deletes `next` from the table, so the script's post-write re-read sees it.
function makeNpm({
  tags,
  failRmTimes = 0,
  failView = false,
  rmError = "E409 conflict",
  ignoreRm = false,
  beforeView,
}) {
  const removed = [];
  const calls = [];
  let views = 0;
  let rmFailuresLeft = failRmTimes;
  return {
    removed,
    calls,
    npm: async (args) => {
      if (args[0] === "view") {
        if (failView) throw new Error("ENOTFOUND registry.npmjs.org");
        views += 1;
        beforeView?.(tags, views);
        return JSON.stringify(tags[args[1]] ?? {});
      }
      if (args[0] === "dist-tag" && args[1] === "rm") {
        calls.push(args);
        if (rmFailuresLeft > 0) {
          rmFailuresLeft -= 1;
          throw new Error(rmError);
        }
        removed.push(args[2]);
        if (!ignoreRm && tags[args[2]]) delete tags[args[2]].next;
        return "";
      }
      throw new Error(`unexpected npm call: ${args.join(" ")}`);
    },
  };
}

const silent = () => {};
const noWait = async () => {};

describe("retireNextDistTag", () => {
  it("removes only the packages whose 'next' is no longer ahead of 'latest'", async () => {
    const { npm, removed } = makeNpm({
      tags: {
        "@takazudo/zfb": { next: "1.1.0-next.1", latest: "2.7.1" },
        "@takazudo/zfb-runtime": { next: "3.0.0-next.1", latest: "2.7.1" },
        "create-zfb": { latest: "2.7.1" },
      },
    });
    const { results, failed } = await retireNextDistTag({
      packages: ["@takazudo/zfb", "@takazudo/zfb-runtime", "create-zfb"],
      npm,
      wait: noWait,
      log: silent,
    });

    expect(removed).toEqual(["@takazudo/zfb"]);
    expect(results.map((r) => r.action)).toEqual(["retired", "keep", "absent"]);
    expect(failed).toBe(0);
  });

  it("writes nothing to the registry in dry-run mode", async () => {
    const { npm, removed } = makeNpm({
      tags: { "@takazudo/zfb": { next: "1.1.0-next.1", latest: "2.7.1" } },
    });
    const { results } = await retireNextDistTag({
      dryRun: true,
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: silent,
    });

    expect(removed).toEqual([]);
    expect(results[0]).toMatchObject({ action: "retire", dryRun: true });
  });

  it("retries a failing removal and succeeds", async () => {
    const { npm, removed } = makeNpm({
      tags: { "@takazudo/zfb": { next: "1.1.0-next.1", latest: "2.7.1" } },
      failRmTimes: 2,
    });
    const { failed } = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: silent,
    });

    expect(removed).toEqual(["@takazudo/zfb"]);
    expect(failed).toBe(0);
  });

  it("reports a permanent failure without rejecting — a release must not go red over a dist-tag", async () => {
    const { npm } = makeNpm({
      tags: { "@takazudo/zfb": { next: "1.1.0-next.1", latest: "2.7.1" } },
      failRmTimes: Number.MAX_SAFE_INTEGER,
    });
    const lines = [];
    const { failed, results } = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: (line) => lines.push(line),
    });

    expect(failed).toBe(1);
    expect(results[0].action).toBe("error");
    expect(lines.some((l) => l.startsWith("::warning::"))).toBe(true);
    expect(lines.some((l) => l.includes("npm dist-tag rm @takazudo/zfb next"))).toBe(true);
  });

  it("survives an unreachable registry", async () => {
    const { npm } = makeNpm({ tags: {}, failView: true });
    const { failed, results } = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: silent,
    });

    expect(failed).toBe(1);
    expect(results[0].action).toBe("error");
  });
});

describe("isTerminalNpmError", () => {
  it("classifies auth failures as not worth retrying", () => {
    expect(isTerminalNpmError("npm error code EOTP")).toBe(true);
    expect(isTerminalNpmError("npm error code ENEEDAUTH")).toBe(true);
    expect(isTerminalNpmError("npm error code E401")).toBe(true);
    expect(isTerminalNpmError("npm error code E403")).toBe(true);
  });

  it("leaves transient failures retryable", () => {
    expect(isTerminalNpmError("E409 conflict")).toBe(false);
    expect(isTerminalNpmError("ENOTFOUND registry.npmjs.org")).toBe(false);
    expect(isTerminalNpmError("")).toBe(false);
    expect(isTerminalNpmError(undefined)).toBe(false);
  });
});

describe("auth failures under OIDC-only auth", () => {
  const TAGS = () => ({
    "@takazudo/zfb": { next: "1.1.0-next.1", latest: "2.7.1" },
    "@takazudo/zfb-runtime": { next: "1.1.0-next.1", latest: "2.7.1" },
  });

  it("stops the sweep on ENEEDAUTH and surfaces it as an ::error:: plus a job summary", async () => {
    const dir = mkdtempSync(join(tmpdir(), "retire-next-"));
    const summaryPath = join(dir, "summary.md");
    try {
      const { npm, calls } = makeNpm({
        tags: TAGS(),
        failRmTimes: Number.MAX_SAFE_INTEGER,
        rmError: "npm error code ENEEDAUTH\nnpm error need auth",
      });
      const lines = [];
      const { results, failed, authFailed } = await retireNextDistTag({
        packages: ["@takazudo/zfb", "@takazudo/zfb-runtime"],
        npm,
        wait: async () => {
          throw new Error("must not back off on a terminal auth error");
        },
        log: (line) => lines.push(line),
        summaryPath,
      });

      expect(calls).toHaveLength(1); // one attempt, on the first package only
      expect(failed).toBe(1);
      expect(authFailed).toBe(true);
      expect(results).toHaveLength(1); // sweep stopped; second package untouched
      expect(results[0]).toMatchObject({ action: "error", terminal: true });
      expect(lines.some((l) => l.startsWith("::error title=npm dist-tag auth failure::"))).toBe(
        true,
      );
      expect(lines.some((l) => l.includes("Allow npm dist-tag"))).toBe(true);
      expect(lines.some((l) => l.includes("--otp"))).toBe(false);
      expect(readFileSync(summaryPath, "utf8")).toMatch(/npm dist-tag auth failure/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("keeps an ordinary (non-auth) failure at ::warning:: severity with no summary", async () => {
    const { npm } = makeNpm({ tags: TAGS(), failRmTimes: Number.MAX_SAFE_INTEGER });
    const lines = [];
    const { authFailed } = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: (line) => lines.push(line),
      summaryPath: undefined,
    });
    expect(authFailed).toBe(false);
    expect(lines.some((l) => l.startsWith("::error"))).toBe(false);
  });

  it("formats a preflight refusal as ::error:: annotations", () => {
    const lines = preflightRefusalLines(["environment variable NODE_AUTH_TOKEN is defined"]);
    expect(lines.every((l) => l.startsWith("::error title=npm dist-tag auth failure::"))).toBe(
      true,
    );
    expect(lines.join("\n")).toMatch(/NODE_AUTH_TOKEN/);
  });

  it("never passes --otp — there is no interactive credential in the release path", async () => {
    const { npm, calls } = makeNpm({ tags: TAGS() });
    await retireNextDistTag({ packages: ["@takazudo/zfb"], npm, wait: noWait, log: silent });
    expect(calls[0]).toEqual(["dist-tag", "rm", "@takazudo/zfb", "next"]);
  });
});

describe("read-compare-write guards", () => {
  it("leaves 'next' alone when a concurrent publish moved it ahead between read and rm", async () => {
    const { npm, calls } = makeNpm({
      tags: { "@takazudo/zfb": { next: "1.1.0-next.1", latest: "2.7.1" } },
      // First view = the sweep's read; second = the re-read right before rm.
      beforeView: (tags, n) => {
        if (n === 2) tags["@takazudo/zfb"].next = "3.0.0-next.1";
      },
    });
    const { results, failed } = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: silent,
    });
    expect(calls).toHaveLength(0);
    expect(failed).toBe(0);
    expect(results[0]).toMatchObject({ action: "changed" });
  });

  it("does not report success when the post-rm re-read still shows 'next'", async () => {
    const { npm } = makeNpm({
      tags: { "@takazudo/zfb": { next: "1.1.0-next.1", latest: "2.7.1" } },
      ignoreRm: true,
    });
    const lines = [];
    const { results, failed } = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: (line) => lines.push(line),
      maxAttempts: 2,
    });
    expect(failed).toBe(1);
    expect(results[0]).toMatchObject({ action: "error", terminal: false });
    expect(lines.some((l) => l.includes("re-read after rm still shows next=1.1.0-next.1"))).toBe(
      true,
    );
  });

  it("confirms a removal by re-reading", async () => {
    const tags = { "@takazudo/zfb": { next: "1.1.0-next.1", latest: "2.7.1" } };
    const { npm } = makeNpm({ tags });
    const lines = [];
    const { results } = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: (line) => lines.push(line),
    });
    expect(results[0].action).toBe("retired");
    expect(tags["@takazudo/zfb"].next).toBeUndefined();
    expect(lines.some((l) => l.includes("confirmed by re-read"))).toBe(true);
  });
});

describe("PUBLISHED_PACKAGES drift guard", () => {
  function workspacePublishableNames() {
    // Mirrors the publishable half of pnpm-workspace.yaml's globs: packages/*
    // and crates/*/npm. `docs` and `examples/*` are private, so a glob-free
    // readdir over these two roots plus a private filter reproduces exactly the
    // set that release.yml publishes.
    const names = [];
    const roots = [
      {
        dir: join(REPO_ROOT, "packages"),
        manifest: (e) => join(REPO_ROOT, "packages", e, "package.json"),
      },
      {
        dir: join(REPO_ROOT, "crates"),
        manifest: (e) => join(REPO_ROOT, "crates", e, "npm", "package.json"),
      },
    ];
    for (const root of roots) {
      for (const entry of readdirSync(root.dir)) {
        const manifestPath = root.manifest(entry);
        if (!existsSync(manifestPath)) continue;
        const pkg = JSON.parse(readFileSync(manifestPath, "utf8"));
        if (pkg.private) continue;
        names.push(pkg.name);
      }
    }
    return names;
  }

  it("covers exactly the workspace's publishable packages", () => {
    expect([...PUBLISHED_PACKAGES].sort()).toEqual(workspacePublishableNames().sort());
  });

  it("matches the package list in advance-latest-dist-tag.sh", () => {
    // The two scripts are the two halves of one invariant: `latest` advances
    // here, `next` retires there. A package present in only one list is the
    // exact drift that froze `next` at 1.1.0-next.1 in the first place.
    const sh = readFileSync(join(REPO_ROOT, "scripts", "advance-latest-dist-tag.sh"), "utf8");
    const tagged = [...sh.matchAll(/^_tag_with_retry "([^"]+)"/gm)].map((m) => m[1]);

    expect(tagged.length).toBeGreaterThan(0);
    expect([...PUBLISHED_PACKAGES].sort()).toEqual([...tagged].sort());
  });
});
