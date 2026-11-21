import { policyFromSourceFiles } from "../release/npm-publication-policy.mjs";
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
// publishable packages and scripts/advance-latest-dist-tag.sh's PACKAGES array.
// The `release-pending` holding-tag sweep (#4717) is covered alongside.

import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

import {
  HOLDING_DIST_TAG,
  PUBLISHED_PACKAGES,
  compareSemver,
  decideNextTagAction,
  decideReleasePendingAction,
  formatSweepSummary,
  isTerminalNpmError,
  parseSemver,
  parseDistTags,
  preflightRefusalLines,
  retireNextDistTag,
} from "../retire-next-dist-tag.mjs";

const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

describe("npm dist-tags JSON shape", () => {
  it("normalizes npm 11 objects and npm 12 single-object arrays", () => {
    const tags = { latest: "7.0.0", "release-pending": "7.0.0" };
    expect(parseDistTags(JSON.stringify(tags))).toEqual(tags);
    expect(parseDistTags(JSON.stringify([tags]))).toEqual(tags);
    expect(parseDistTags("{}")).toEqual({});
    expect(parseDistTags("[{}]")).toEqual({});
  });
  it.each([
    "",
    "[]",
    "[{},{}]",
    "[[]]",
    "null",
    "[null]",
    "1",
    '"tags"',
    '{"latest":7}',
    '{"release-pending":""}',
    "not-json",
  ])("refuses malformed or ambiguous evidence %s", async (raw) => {
    expect(() => parseDistTags(raw)).toThrow();
    const calls = [];
    const report = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm: async (args) => {
        calls.push(args);
        return raw;
      },
      wait: async () => {},
      log: () => {},
    });
    expect(report.failed).toBe(1);
    expect(report.pendingResults[0].action).toBe("error");
    expect(calls.every((args) => args[0] === "view")).toBe(true);
  });
});

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

describe("decideReleasePendingAction (#4717)", () => {
  it("is not a semver range, so npm never treats it as a version", () => {
    expect(HOLDING_DIST_TAG).toBe("release-pending");
    expect(parseSemver(HOLDING_DIST_TAG)).toBeNull();
  });

  it("retires a holding tag the stable channel has reached", () => {
    expect(decideReleasePendingAction({ latest: "6.1.3", "release-pending": "6.1.3" }).action).toBe(
      "retire",
    );
    expect(decideReleasePendingAction({ latest: "6.1.4", "release-pending": "6.1.3" }).action).toBe(
      "retire",
    );
  });

  it("keeps a holding tag ahead of latest — a release in flight or still processing", () => {
    const decision = decideReleasePendingAction({ latest: "6.1.2", "release-pending": "6.1.3" });
    expect(decision.action).toBe("keep");
    expect(decision.reason).toMatch(/in flight/);
  });

  it("retires a prerelease holding tag that next has reached", () => {
    expect(
      decideReleasePendingAction({
        latest: "6.1.3",
        next: "7.0.0-next.2",
        "release-pending": "7.0.0-next.2",
      }).action,
    ).toBe("retire");
  });

  it("never lets a prerelease soak on next retire a stable holding version", () => {
    expect(
      decideReleasePendingAction({
        latest: "6.1.2",
        next: "7.0.0-next.2",
        "release-pending": "6.1.3",
      }).action,
    ).toBe("keep");
  });

  it("keeps a prerelease holding tag ahead of both channels", () => {
    expect(
      decideReleasePendingAction({
        latest: "6.1.3",
        next: "7.0.0-next.1",
        "release-pending": "7.0.0-next.2",
      }).action,
    ).toBe("keep");
  });

  it("refuses to delete on a guess", () => {
    expect(decideReleasePendingAction({ latest: "6.1.3", "release-pending": "junk" }).action).toBe(
      "keep",
    );
    expect(decideReleasePendingAction({ latest: "junk", "release-pending": "6.1.3" }).action).toBe(
      "keep",
    );
  });

  it("reports absent when there is no holding tag", () => {
    expect(decideReleasePendingAction({ latest: "6.1.3" }).action).toBe("absent");
  });
});

// Fake `npm` runner: answers `view ... dist-tags --json` from a mutable table
// and records every `dist-tag rm` it is asked to perform. A successful rm
// deletes the named tag from the table, so the script's post-write re-read sees it.
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
        if (!ignoreRm && tags[args[2]]) delete tags[args[2]][args[3]];
        return "";
      }
      throw new Error(`unexpected npm call: ${args.join(" ")}`);
    },
  };
}

const silent = () => {};
const noWait = async () => {};

describe("retireNextDistTag", () => {
  it("removes and confirms both stale tags with npm 12 array-shaped reads", async () => {
    const fake = makeNpm({
      tags: {
        "@takazudo/zfb": { latest: "7.0.0", next: "6.0.0-next.1", "release-pending": "7.0.0" },
      },
    });
    const npm = async (args) => {
      const result = await fake.npm(args);
      return args[0] === "view" ? JSON.stringify([JSON.parse(result)]) : result;
    };
    const report = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: silent,
    });
    expect(report.failed).toBe(0);
    expect(report.results[0].action).toBe("retired");
    expect(report.pendingResults[0].action).toBe("retired");
    expect(fake.calls).toEqual([
      ["dist-tag", "rm", "@takazudo/zfb", "next"],
      ["dist-tag", "rm", "@takazudo/zfb", "release-pending"],
    ]);
  });
  it("source policy excludes suspended carrier from both cleanup writes", async () => {
    const packages = policyFromSourceFiles({
      policyText: JSON.stringify({
        schema: "zfb-npm-publication-policy/v1",
        suspended: ["@takazudo/zfb-linux-arm64-gnu"],
      }),
    }).names;
    const tags = Object.fromEntries(
      PUBLISHED_PACKAGES.map((name) => [
        name,
        { latest: "7.0.0", next: "6.0.0-next.1", "release-pending": "7.0.0" },
      ]),
    );
    const { npm, calls } = makeNpm({ tags });
    await retireNextDistTag({ packages, npm, wait: noWait, log: silent });
    expect(calls).toHaveLength(20);
    expect(calls.some((args) => args.includes("@takazudo/zfb-linux-arm64-gnu"))).toBe(false);
    expect(tags["@takazudo/zfb-linux-arm64-gnu"]).toEqual({
      latest: "7.0.0",
      next: "6.0.0-next.1",
      "release-pending": "7.0.0",
    });
  });
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

describe("release-pending sweep (#4717)", () => {
  it("removes release-pending at or below the channel and leaves a newer one alone", async () => {
    const tags = {
      "@takazudo/zfb": { latest: "6.1.3", "release-pending": "6.1.3" },
      "@takazudo/zfb-linux-arm64-gnu": { latest: "6.1.3", "release-pending": "6.1.2" },
      "create-zfb": { latest: "6.1.3", "release-pending": "6.1.4" },
      "@takazudo/zfb-runtime": { latest: "6.1.3" },
    };
    const { npm, calls } = makeNpm({ tags });
    const { pendingResults, results, failed } = await retireNextDistTag({
      packages: Object.keys(tags),
      npm,
      wait: noWait,
      log: silent,
    });
    expect(calls).toEqual([
      ["dist-tag", "rm", "@takazudo/zfb", HOLDING_DIST_TAG],
      ["dist-tag", "rm", "@takazudo/zfb-linux-arm64-gnu", HOLDING_DIST_TAG],
    ]);
    expect(pendingResults.map((r) => r.action)).toEqual(["retired", "retired", "keep", "absent"]);
    expect(results.map((r) => r.action)).toEqual(["absent", "absent", "absent", "absent"]);
    expect(tags["create-zfb"][HOLDING_DIST_TAG]).toBe("6.1.4");
    expect(tags["@takazudo/zfb"].latest).toBe("6.1.3");
    expect(failed).toBe(0);
  });

  it("sweeps both next and release-pending once a prerelease graduates", async () => {
    const tags = {
      "@takazudo/zfb": { latest: "7.0.0", next: "7.0.0-rc.1", "release-pending": "7.0.0" },
    };
    const { npm, calls } = makeNpm({ tags });
    await retireNextDistTag({ packages: ["@takazudo/zfb"], npm, wait: noWait, log: silent });
    expect(calls).toEqual([
      ["dist-tag", "rm", "@takazudo/zfb", "next"],
      ["dist-tag", "rm", "@takazudo/zfb", HOLDING_DIST_TAG],
    ]);
    expect(tags["@takazudo/zfb"]).toEqual({ latest: "7.0.0" });
  });

  it("re-decides on fresh state: a holding tag moved ahead before the rm is kept", async () => {
    const { npm, calls } = makeNpm({
      tags: { "@takazudo/zfb": { latest: "6.1.3", "release-pending": "6.1.3" } },
      beforeView: (tags, n) => {
        if (n === 2) tags["@takazudo/zfb"][HOLDING_DIST_TAG] = "6.1.4";
      },
    });
    const { pendingResults } = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: silent,
    });
    expect(calls).toHaveLength(0);
    expect(pendingResults[0]).toMatchObject({ action: "changed" });
  });

  it("writes nothing in dry-run mode", async () => {
    const { npm, calls } = makeNpm({
      tags: { "@takazudo/zfb": { latest: "6.1.3", "release-pending": "6.1.3" } },
    });
    const { pendingResults } = await retireNextDistTag({
      dryRun: true,
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: silent,
    });
    expect(calls).toEqual([]);
    expect(pendingResults[0]).toMatchObject({ action: "retire", dryRun: true });
  });

  it("reports a permanent holding-tag failure without rejecting", async () => {
    const lines = [];
    const { npm } = makeNpm({
      tags: { "@takazudo/zfb": { latest: "6.1.3", "release-pending": "6.1.3" } },
      failRmTimes: Number.MAX_SAFE_INTEGER,
    });
    const { failed, pendingResults } = await retireNextDistTag({
      packages: ["@takazudo/zfb"],
      npm,
      wait: noWait,
      log: (line) => lines.push(line),
    });
    expect(failed).toBe(1);
    expect(pendingResults[0].action).toBe("error");
    expect(lines.some((l) => l.includes(`npm dist-tag rm @takazudo/zfb ${HOLDING_DIST_TAG}`))).toBe(
      true,
    );
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

// A fully scripted registry: each npm call consumes the next step, and every
// invocation (reads included) is recorded so the sequence itself is asserted.
function scriptedNpm(steps) {
  const invocations = [];
  let index = 0;
  return {
    invocations,
    remaining: () => steps.length - index,
    npm: async (args) => {
      invocations.push(args);
      const step = steps[index];
      index += 1;
      if (!step) throw new Error(`unscripted npm call: ${args.join(" ")}`);
      expect(args[0] === "view" ? "view" : "rm").toBe(step.kind);
      if (step.error) throw new Error(step.error);
      return step.kind === "view" ? JSON.stringify(step.tags) : "";
    },
  };
}

describe("settled-absence reporting (#4769)", () => {
  const PKG = "@takazudo/zfb-darwin-arm64";
  const VIEW = ["view", PKG, "dist-tags", "--json", "--prefer-online"];
  const RM = (tag) => ["dist-tag", "rm", PKG, tag];
  const view = (tags) => ({ kind: "view", tags });
  const rm = (error) => ({ kind: "rm", error });

  async function run(steps, options = {}) {
    const fake = scriptedNpm(steps);
    const lines = [];
    const waits = [];
    const report = await retireNextDistTag({
      packages: [PKG],
      npm: fake.npm,
      wait: async (ms) => {
        waits.push(ms);
      },
      log: (line) => lines.push(line),
      summaryPath: undefined,
      ...options,
    });
    expect(fake.remaining()).toBe(0);
    return { ...report, lines, waits, invocations: fake.invocations };
  }

  const summary = (report, dryRun = false) => formatSweepSummary({ ...report, dryRun });

  for (const [tag, stale] of [
    [HOLDING_DIST_TAG, { latest: "7.1.0", [HOLDING_DIST_TAG]: "7.1.0" }],
    ["next", { latest: "7.1.0", next: "7.1.0-rc.1" }],
  ]) {
    const gone = { latest: "7.1.0" };

    describe(`'${tag}'`, () => {
      it("immediate confirmation is the only outcome counted as retired", async () => {
        const report = await run([view(stale), view(stale), rm(), view(gone)]);
        const result = (tag === "next" ? report.results : report.pendingResults)[0];
        expect(result).toMatchObject({
          action: "retired",
          writeAttempts: 1,
          lastWrite: "acknowledged",
          observed: "absent",
        });
        expect(report.invocations).toEqual([VIEW, VIEW, RM(tag), VIEW]);
        expect(report.lines.some((l) => l.includes("confirmed by re-read (attempt 1)"))).toBe(true);
        expect(report.lines.some((l) => l.includes("in place"))).toBe(false);
        expect(report.failed).toBe(0);
        expect(summary(report)).toBe(
          tag === "next"
            ? `dist-tag sweep complete: 'next' 1 retired, 0 absent after attempt; '${HOLDING_DIST_TAG}' 0 retired, 0 absent after attempt; 0 failed.`
            : `dist-tag sweep complete: 'next' 0 retired, 0 absent after attempt; '${HOLDING_DIST_TAG}' 1 retired, 0 absent after attempt; 0 failed.`,
        );
      });

      it("delayed confirmation then absence (the v7.1.0 finalizer sequence) is not 'left in place'", async () => {
        const report = await run([
          view(stale), // sweep pre-filter
          view(stale), // attempt 1 pre-write read
          rm(), // acknowledged
          view(stale), // confirm read still stale
          view(stale), // attempt 2 pre-write read, still stale
          rm(`Command failed: npm dist-tag rm\nnpm error ${tag} is not a dist-tag on ${PKG}`),
          view(gone), // attempt 3 pre-write read: absent
        ]);
        const result = (tag === "next" ? report.results : report.pendingResults)[0];
        expect(result).toEqual({
          pkg: PKG,
          action: "absent-after-attempt",
          reason: `no '${tag}' dist-tag`,
          writeAttempts: 2,
          lastWrite: "failed",
          observed: "absent",
        });
        expect(report.invocations).toEqual([VIEW, VIEW, RM(tag), VIEW, VIEW, RM(tag), VIEW]);
        expect(report.waits).toEqual([5000, 10000]);
        expect(report.lines.some((l) => l.includes("in place"))).toBe(false);
        expect(report.lines.some((l) => l.startsWith("::"))).toBe(false);
        expect(
          report.lines.filter((l) => l.includes(`dist-tag rm ${tag} not confirmed`)),
        ).toHaveLength(2);
        expect(report.lines).toContain(
          `  ${PKG}: '${tag}' observed absent on a later re-read after 2 removal attempt(s) (last rm failed) — absence settled; removal not attributed to this run.`,
        );
        expect(report.failed).toBe(0);
        expect(summary(report)).toBe(
          tag === "next"
            ? `dist-tag sweep complete: 'next' 0 retired, 1 absent after attempt; '${HOLDING_DIST_TAG}' 0 retired, 0 absent after attempt; 0 failed.`
            : `dist-tag sweep complete: 'next' 0 retired, 0 absent after attempt; '${HOLDING_DIST_TAG}' 0 retired, 1 absent after attempt; 0 failed.`,
        );
      });

      it("absence before any write is reported as absent, never as a removal", async () => {
        const report = await run([view(stale), view(gone)]);
        const result = (tag === "next" ? report.results : report.pendingResults)[0];
        expect(result).toMatchObject({
          action: "absent",
          writeAttempts: 0,
          lastWrite: "none",
          observed: "absent",
        });
        expect(report.invocations).toEqual([VIEW, VIEW]);
        expect(report.lines).toContain(
          `  ${PKG}: '${tag}' already absent before any removal attempt (no '${tag}' dist-tag).`,
        );
        expect(report.lines.some((l) => l.includes("in place"))).toBe(false);
        expect(summary(report)).toMatch(/0 retired, 0 absent after attempt; .*0 retired, 0 absent/);
      });

      it("a concurrent move ahead is the only outcome reported as left in place", async () => {
        const ahead =
          tag === "next"
            ? { latest: "7.1.0", next: "7.2.0-next.1" }
            : { latest: "7.1.0", [HOLDING_DIST_TAG]: "7.1.1" };
        const report = await run([view(stale), view(ahead)]);
        const result = (tag === "next" ? report.results : report.pendingResults)[0];
        expect(result).toMatchObject({
          action: "changed",
          writeAttempts: 0,
          lastWrite: "none",
          observed: "retained",
        });
        expect(report.invocations).toEqual([VIEW, VIEW]);
        expect(
          report.lines.some((l) =>
            l.startsWith(`  ${PKG}: left '${tag}' in place — dist-tags changed before the write`),
          ),
        ).toBe(true);
        expect(report.failed).toBe(0);
        expect(summary(report)).not.toMatch(/[1-9] (retired|absent after attempt)/);
      });

      it("a terminal auth failure stops with ::error:: and is not counted as absent", async () => {
        const report = await run([view(stale), view(stale), rm("npm error code E403")]);
        const result = (tag === "next" ? report.results : report.pendingResults)[0];
        expect(result).toMatchObject({
          action: "error",
          terminal: true,
          writeAttempts: 1,
          lastWrite: "failed",
          observed: "unknown",
        });
        expect(report.authFailed).toBe(true);
        expect(report.waits).toEqual([]);
        expect(report.invocations).toEqual([VIEW, VIEW, RM(tag)]);
        expect(
          report.lines.some((l) => l.startsWith("::error title=npm dist-tag auth failure::")),
        ).toBe(true);
        expect(summary(report)).toMatch(/; 1 failed\.$/);
        expect(summary(report)).not.toMatch(/[1-9] (retired|absent after attempt)/);
      });

      it("persistent stale confirmation fails after maxAttempts at ::warning::", async () => {
        const steps = [view(stale)];
        for (let i = 0; i < 3; i += 1) steps.push(view(stale), rm(), view(stale));
        const report = await run(steps, { maxAttempts: 3 });
        const result = (tag === "next" ? report.results : report.pendingResults)[0];
        expect(result).toMatchObject({
          action: "error",
          terminal: false,
          writeAttempts: 3,
          lastWrite: "acknowledged",
          observed: "present",
        });
        expect(report.waits).toEqual([5000, 10000]);
        expect(report.invocations.filter((a) => a[0] === "dist-tag")).toHaveLength(3);
        expect(report.lines.some((l) => l.startsWith("::warning::"))).toBe(true);
        expect(report.lines.some((l) => l.startsWith("::error"))).toBe(false);
        expect(summary(report)).toMatch(/0 retired, 0 absent after attempt; .*; 1 failed\.$/);
      });

      it("dry run decides 'would remove' and writes nothing", async () => {
        const report = await run([view(stale)], { dryRun: true });
        const result = (tag === "next" ? report.results : report.pendingResults)[0];
        expect(result).toMatchObject({ action: "retire", dryRun: true });
        expect(report.invocations).toEqual([VIEW]);
        expect(report.lines).toContain(`  ${PKG}: would remove '${tag}' — ${result.reason}`);
        expect(summary(report, true)).toBe(
          tag === "next"
            ? `dist-tag sweep complete: 'next' 1 would retire; '${HOLDING_DIST_TAG}' 0 would retire; 0 failed.`
            : `dist-tag sweep complete: 'next' 0 would retire; '${HOLDING_DIST_TAG}' 1 would retire; 0 failed.`,
        );
      });
    });
  }

  it("counts the all-packages finalizer sequence as settled absence, not 0 of 11", async () => {
    const stale = { latest: "7.1.0", [HOLDING_DIST_TAG]: "7.1.0" };
    const gone = { latest: "7.1.0" };
    const packages = PUBLISHED_PACKAGES;
    const steps = packages.flatMap((pkg) => [
      view(stale),
      view(stale),
      rm(),
      view(stale),
      view(stale),
      rm(`npm error ${HOLDING_DIST_TAG} is not a dist-tag on ${pkg}`),
      view(gone),
    ]);
    const report = await run(steps, { packages });
    expect(report.pendingResults.map((r) => r.action)).toEqual(
      packages.map(() => "absent-after-attempt"),
    );
    expect(report.results.map((r) => r.action)).toEqual(packages.map(() => "absent"));
    expect(summary(report)).toBe(
      `dist-tag sweep complete: 'next' 0 retired, 0 absent after attempt; '${HOLDING_DIST_TAG}' 0 retired, ${packages.length} absent after attempt; 0 failed.`,
    );
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
    expect(sh).toContain("release/source-publication-policy.mjs");
    expect(sh).toContain("ZFB_RELEASE_SOURCE_COMMIT");
    expect(sh).not.toContain('"@takazudo/zfb-linux-arm64-gnu"');
  });
});
