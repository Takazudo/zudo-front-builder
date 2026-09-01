import { describe, expect, it } from "vitest";

import {
  CANDIDATE_DRIFT,
  NO_DRIFT,
  OPERATIONAL_FAILURE,
  TRACKED_CANDIDATES,
  compareCandidate,
  compareSnapshots,
  formatJsonReport,
  formatReport,
} from "../check-yaml-candidate-drift.mjs";

function candidate(overrides = {}) {
  return {
    crate: "noyalib",
    repo: "noyato/noyalib",
    versions: ["0.0.28"],
    branches: {
      main: "1111111",
      "feat/v0.0.29": "697195f",
    },
    tags: ["v0.0.28"],
    releases: ["v0.0.28"],
    pendingReleasePr: { number: 365, state: "OPEN" },
    archived: false,
    checkedAt: "2026-08-31T17:36:36Z",
    ...overrides,
  };
}

function snapshots(changedName, changedCandidate) {
  const candidates = Object.fromEntries(
    TRACKED_CANDIDATES.map((name) => [name, candidate({ crate: name, repo: `owner/${name}` })]),
  );
  return {
    baseline: {
      schemaVersion: 1,
      checkedAt: "2026-08-31T17:36:36Z",
      candidates,
    },
    observed: {
      schemaVersion: 1,
      checkedAt: "2026-09-01T00:00:00Z",
      candidates: {
        ...structuredClone(candidates),
        ...(changedName ? { [changedName]: changedCandidate } : {}),
      },
    },
  };
}

describe("compareCandidate", () => {
  it("reports no-drift when only the checkedAt cutoff moved", () => {
    expect(
      compareCandidate("noyalib", candidate(), candidate({ checkedAt: "2026-09-01T00:00:00Z" })),
    ).toEqual({
      name: "noyalib",
      status: NO_DRIFT,
      deltas: [],
    });
  });

  it("detects any newly published version, even a semver backport below the maximum", () => {
    const result = compareCandidate(
      "noyalib",
      candidate({ versions: ["1.0.0"] }),
      candidate({ versions: ["0.9.9", "1.0.0"] }),
    );
    expect(result.deltas).toContainEqual({ kind: "version-published", version: "0.9.9" });
  });

  it("treats an observation that omits a known published version as incomplete", () => {
    const result = compareCandidate(
      "noyalib",
      candidate({ versions: ["0.0.27", "0.0.28"] }),
      candidate({ versions: ["0.0.28"] }),
    );
    expect(result).toMatchObject({
      status: OPERATIONAL_FAILURE,
      error: expect.stringContaining("0.0.27"),
    });
  });

  it("detects a new commit on a non-default release branch", () => {
    // Regression pin for the 2026-08-31 false negative: looking only at main
    // would miss this exact class of candidate movement.
    const observed = candidate({
      branches: {
        main: "1111111",
        "feat/v0.0.29": { sha: "8888888", relation: "ahead" },
      },
    });
    const result = compareCandidate("noyalib", candidate(), observed);
    expect(result.status).toBe(CANDIDATE_DRIFT);
    expect(result.deltas).toContainEqual({
      kind: "branch-advanced",
      branch: "feat/v0.0.29",
      from: "697195f",
      to: "8888888",
    });
  });

  it.each([
    [
      "new branch",
      { main: "1111111", "feat/v0.0.29": "697195f", release: "2222222" },
      { kind: "branch-added", branch: "release", sha: "2222222" },
    ],
    [
      "deleted branch",
      { main: "1111111" },
      { kind: "branch-deleted", branch: "feat/v0.0.29", sha: "697195f" },
    ],
    [
      "force-pushed/diverged branch",
      {
        main: "1111111",
        "feat/v0.0.29": { sha: "3333333", relation: "diverged" },
      },
      {
        kind: "branch-diverged",
        branch: "feat/v0.0.29",
        from: "697195f",
        to: "3333333",
      },
    ],
  ])("distinguishes a %s", (_label, branches, expected) => {
    const result = compareCandidate("noyalib", candidate(), candidate({ branches }));
    expect(result.status).toBe(CANDIDATE_DRIFT);
    expect(result.deltas).toContainEqual(expected);
  });

  it("fails operationally rather than guessing when a changed head lacks ancestry evidence", () => {
    const result = compareCandidate(
      "noyalib",
      candidate(),
      candidate({ branches: { main: "1111111", "feat/v0.0.29": "3333333" } }),
    );
    expect(result.status).toBe(OPERATIONAL_FAILURE);
    expect(result.error).toMatch(/without ancestry evidence/);
  });

  it("detects new tags and GitHub Releases independently", () => {
    const result = compareCandidate(
      "noyalib",
      candidate(),
      candidate({ tags: ["v0.0.28", "v0.0.29"], releases: ["v0.0.28", "v0.0.29"] }),
    );
    expect(result.deltas).toEqual(
      expect.arrayContaining([
        { kind: "tag-added", tag: "v0.0.29" },
        { kind: "release-added", release: "v0.0.29" },
      ]),
    );
  });

  it.each(["MERGED", "CLOSED"])("distinguishes pending release PR OPEN -> %s", (state) => {
    const result = compareCandidate(
      "noyalib",
      candidate(),
      candidate({ pendingReleasePr: { number: 365, state } }),
    );
    expect(result.deltas).toContainEqual({
      kind: "release-pr-state-changed",
      number: 365,
      from: "OPEN",
      to: state,
    });
  });

  it.each([
    [false, true, "repository-archived"],
    [true, false, "repository-unarchived"],
  ])("detects archive state %s -> %s", (before, after, kind) => {
    const result = compareCandidate(
      "noyalib",
      candidate({ archived: before }),
      candidate({ archived: after }),
    );
    expect(result.deltas).toContainEqual({ kind });
  });
});

describe("compareSnapshots", () => {
  it("returns the stable no-drift JSON shape and exit code 0", () => {
    const { baseline, observed } = snapshots();
    const result = compareSnapshots(baseline, observed);
    expect(result).toMatchObject({
      status: NO_DRIFT,
      exitCode: 0,
      checkedAt: observed.checkedAt,
      errors: [],
    });
    expect(result.candidates).toHaveLength(TRACKED_CANDIDATES.length);
  });

  it("returns exit code 10 for CANDIDATE_DRIFT", () => {
    const changed = candidate({
      crate: "noyalib",
      repo: "owner/noyalib",
      versions: ["0.0.28", "0.0.29"],
    });
    const { baseline, observed } = snapshots("noyalib", changed);
    expect(compareSnapshots(baseline, observed)).toMatchObject({
      status: CANDIDATE_DRIFT,
      exitCode: 10,
    });
  });

  it("gives operational failure precedence when another candidate has drift", () => {
    const changed = candidate({
      crate: "noyalib",
      repo: "owner/noyalib",
      versions: ["0.0.28", "0.0.29"],
    });
    const { baseline, observed } = snapshots("noyalib", changed);
    observed.candidates.serde_yml = { error: "crates.io responded 503" };
    const result = compareSnapshots(baseline, observed);
    expect(result.status).toBe(OPERATIONAL_FAILURE);
    expect(result.exitCode).not.toBe(0);
    expect(result.exitCode).not.toBe(10);
    expect(result.candidates.find((item) => item.name === "noyalib").status).toBe(CANDIDATE_DRIFT);
    expect(result.errors).toContainEqual({
      candidate: "serde_yml",
      message: "crates.io responded 503",
    });
  });

  it("treats an omitted candidate as an operational failure, never no-drift", () => {
    const { baseline, observed } = snapshots();
    delete observed.candidates.saphyr;
    expect(compareSnapshots(baseline, observed)).toMatchObject({
      status: OPERATIONAL_FAILURE,
      exitCode: 1,
    });
  });
});

describe("report formatters", () => {
  it("uses CANDIDATE_DRIFT vocabulary and does not claim the trigger fired", () => {
    const changed = candidate({
      crate: "noyalib",
      repo: "owner/noyalib",
      pendingReleasePr: { number: 365, state: "MERGED" },
    });
    const { baseline, observed } = snapshots("noyalib", changed);
    const report = formatReport(compareSnapshots(baseline, observed));
    expect(report).toContain("CANDIDATE_DRIFT");
    expect(report).toContain("release PR #365 OPEN -> MERGED");
    expect(report).not.toMatch(/trigger fired/i);
  });

  it("renders the --json shape as parseable newline-terminated JSON", () => {
    const { baseline, observed } = snapshots();
    const result = compareSnapshots(baseline, observed);
    const json = formatJsonReport(result);
    expect(json.endsWith("\n")).toBe(true);
    expect(JSON.parse(json)).toEqual(result);
  });
});
