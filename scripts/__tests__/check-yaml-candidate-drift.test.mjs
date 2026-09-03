import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import {
  BASELINE_COMMENT,
  CANDIDATE_CONFIG,
  CANDIDATE_DRIFT,
  DELTA_KINDS,
  EXIT_CODES,
  INFORMATIONAL,
  INFORMATIONAL_DRIFT,
  NO_DRIFT,
  OPERATIONAL_FAILURE,
  TRACKED_CANDIDATES,
  TRIAGE,
  compareCandidate,
  compareSnapshots,
  createNetworkClients,
  deltaSeverity,
  fetchJson,
  formatJsonReport,
  formatReport,
  observeSnapshot,
  parseCliArgs,
  runCli,
  snapshotForBaseline,
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

const DOCUMENTED_DELTA_KINDS = [
  "branch-added",
  "branch-advanced",
  "branch-deleted",
  "branch-diverged",
  "release-added",
  "release-pr-changed",
  "release-pr-state-changed",
  "repository-archived",
  "repository-unarchived",
  "tag-added",
  "version-published",
];

function informationalNoyalib() {
  return candidate({
    crate: "noyalib",
    repo: "owner/noyalib",
    branches: {
      main: "1111111",
      "feat/v0.0.29": { sha: "8888888", relation: "ahead" },
    },
  });
}

describe("severity classification", () => {
  it("maps every status to its documented exit code", () => {
    for (const status of [NO_DRIFT, INFORMATIONAL_DRIFT, CANDIDATE_DRIFT, OPERATIONAL_FAILURE]) {
      expect(Number.isInteger(EXIT_CODES[status])).toBe(true);
    }
    expect(EXIT_CODES[NO_DRIFT]).toBe(0);
    expect(EXIT_CODES[INFORMATIONAL_DRIFT]).toBe(0);
    expect(EXIT_CODES[CANDIDATE_DRIFT]).toBe(10);
    expect(EXIT_CODES[OPERATIONAL_FAILURE]).toBe(1);
  });

  it("classifies exactly the documented delta kinds and fails closed on any other", () => {
    expect(Object.keys(DELTA_KINDS).sort()).toEqual(DOCUMENTED_DELTA_KINDS);
    for (const kind of DOCUMENTED_DELTA_KINDS) {
      expect(deltaSeverity(kind)).toBe(kind.startsWith("branch-") ? INFORMATIONAL : TRIAGE);
    }
    expect(() => deltaSeverity("mystery")).toThrow(/unknown delta kind/);
    expect(() =>
      formatReport({
        status: CANDIDATE_DRIFT,
        exitCode: 10,
        checkedAt: "2026-09-01T00:00:00Z",
        candidates: [
          {
            name: "noyalib",
            role: "adopted",
            status: CANDIDATE_DRIFT,
            deltas: [{ kind: "mystery" }],
          },
        ],
        errors: [],
      }),
    ).toThrow(/unknown delta kind/);
  });
});

describe("compareCandidate", () => {
  it("reports no-drift when only the checkedAt cutoff moved", () => {
    expect(
      compareCandidate("noyalib", candidate(), candidate({ checkedAt: "2026-09-01T00:00:00Z" })),
    ).toEqual({
      name: "noyalib",
      role: "adopted",
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
    expect(result.status).toBe(INFORMATIONAL_DRIFT);
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
    expect(result.status).toBe(INFORMATIONAL_DRIFT);
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

  it("keeps every flavour of branch-only movement informational", () => {
    const result = compareCandidate(
      "noyalib",
      candidate({ branches: { main: "1111111", "feat/v0.0.29": "697195f", legacy: "5555555" } }),
      candidate({
        branches: {
          main: { sha: "9999999", relation: "ahead" },
          "feat/v0.0.29": { sha: "3333333", relation: "diverged" },
          release: "2222222",
        },
      }),
    );
    expect(result.status).toBe(INFORMATIONAL_DRIFT);
    expect(result.deltas.map((delta) => delta.kind).sort()).toEqual([
      "branch-added",
      "branch-advanced",
      "branch-deleted",
      "branch-diverged",
    ]);
  });

  it("raises a mixed delta list to CANDIDATE_DRIFT without filtering the informational deltas", () => {
    const result = compareCandidate(
      "noyalib",
      candidate(),
      candidate({
        versions: ["0.0.28", "0.0.29"],
        branches: { main: "1111111", "feat/v0.0.29": { sha: "8888888", relation: "ahead" } },
      }),
    );
    expect(result.status).toBe(CANDIDATE_DRIFT);
    expect(result.deltas).toContainEqual({ kind: "version-published", version: "0.0.29" });
    expect(result.deltas).toContainEqual({
      kind: "branch-advanced",
      branch: "feat/v0.0.29",
      from: "697195f",
      to: "8888888",
    });
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

  it("returns informational-drift and exit code 0 for branch-only movement", () => {
    const { baseline, observed } = snapshots("noyalib", informationalNoyalib());
    const result = compareSnapshots(baseline, observed);
    expect(result).toMatchObject({ status: INFORMATIONAL_DRIFT, exitCode: 0, errors: [] });
    expect(result.candidates.find((item) => item.name === "noyalib")).toMatchObject({
      role: "adopted",
      status: INFORMATIONAL_DRIFT,
    });
  });

  it("gives a triage-severity delta on any candidate precedence over informational drift", () => {
    const { baseline, observed } = snapshots("noyalib", informationalNoyalib());
    observed.candidates.saphyr = candidate({
      crate: "saphyr",
      repo: "owner/saphyr",
      versions: ["0.0.28", "0.0.29"],
    });
    const result = compareSnapshots(baseline, observed);
    expect(result).toMatchObject({ status: CANDIDATE_DRIFT, exitCode: 10 });
    expect(result.candidates.find((item) => item.name === "noyalib").status).toBe(
      INFORMATIONAL_DRIFT,
    );
  });

  it("gives an operational failure precedence over informational drift", () => {
    const { baseline, observed } = snapshots("noyalib", informationalNoyalib());
    observed.candidates.serde_yml = { error: "crates.io responded 503" };
    expect(compareSnapshots(baseline, observed)).toMatchObject({
      status: OPERATIONAL_FAILURE,
      exitCode: 1,
    });
  });

  it("echoes the configured role onto failure rows, and null onto the unknown-candidate row", () => {
    const { baseline, observed } = snapshots();
    observed.candidates.noyalib = { error: "crates.io responded 503" };
    delete observed.candidates.saphyr;
    baseline.candidates.mystery_fork = candidate();
    const rows = Object.fromEntries(
      compareSnapshots(baseline, observed).candidates.map((row) => [row.name, row]),
    );
    expect(rows.noyalib).toMatchObject({ role: "adopted", status: OPERATIONAL_FAILURE });
    expect(rows.saphyr).toMatchObject({ role: "candidate", status: OPERATIONAL_FAILURE });
    expect(rows.baseline).toMatchObject({ role: null, status: OPERATIONAL_FAILURE });
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
    expect(report).toContain("adopted dependency");
    expect(report).toContain("[triage] release PR #365 OPEN -> MERGED");
    expect(report).not.toMatch(/trigger fired/i);
  });

  it("renders the --json shape as parseable newline-terminated JSON", () => {
    const { baseline, observed } = snapshots();
    const result = compareSnapshots(baseline, observed);
    const json = formatJsonReport(result);
    expect(json.endsWith("\n")).toBe(true);
    expect(JSON.parse(json)).toEqual(result);
  });

  it("never dresses branch-only movement as CANDIDATE_DRIFT", () => {
    const { baseline, observed } = snapshots("noyalib", informationalNoyalib());
    const report = formatReport(compareSnapshots(baseline, observed));
    expect(report).toContain("informational-drift (branch movement only; no triage required)");
    expect(report).toContain("[informational] branch");
    expect(report).toContain("no triage, no tracking issue, and no baseline refresh");
    expect(report).not.toContain("CANDIDATE_DRIFT");
    expect(report).not.toMatch(/trigger fired/i);
  });

  it("names the re-scan protocol for a candidate-role crate and the harness in the footer", () => {
    const changed = candidate({
      crate: "saphyr",
      repo: "owner/saphyr",
      versions: ["0.0.28", "0.0.29"],
    });
    const { baseline, observed } = snapshots("saphyr", changed);
    const report = formatReport(compareSnapshots(baseline, observed));
    expect(report).toContain("CANDIDATE_DRIFT (candidate: re-scan against #2755)");
    expect(report).toContain("[triage] new crates.io version 0.0.29");
    expect(report).toContain("yaml_differential_harness.rs");
    expect(report).not.toMatch(/trigger fired/i);
  });

  it("still renders a saved report from before roles existed", () => {
    const { baseline, observed } = snapshots(
      "saphyr",
      candidate({ crate: "saphyr", repo: "owner/saphyr", versions: ["0.0.28", "0.0.29"] }),
    );
    const result = compareSnapshots(baseline, observed);
    for (const row of result.candidates) delete row.role;
    const report = formatReport(result);
    expect(report).toContain("CANDIDATE_DRIFT (candidate: re-scan against #2755)");
    expect(report).not.toContain("undefined");
  });
});

function headers(values = {}) {
  return new Headers(values);
}

function response(data, { status = 200, responseHeaders = {} } = {}) {
  return new Response(JSON.stringify(data), {
    status,
    statusText: status === 200 ? "OK" : "failure",
    headers: { "content-type": "application/json", ...responseHeaders },
  });
}

function completeClients(overrides = {}) {
  return {
    crateVersions: async () => ["1.0.0"],
    repo: async () => ({ archived: false }),
    branches: async () => [{ name: "main", commit: { sha: "abc" } }],
    tags: async () => [{ name: "v1.0.0" }],
    releases: async () => [{ tag_name: "v1.0.0" }],
    pullRequest: async () => ({ state: "open", merged_at: null }),
    compare: async () => ({ status: "ahead" }),
    ...overrides,
  };
}

function throwingClients() {
  const fail = async () => {
    throw new Error("--render must not reach the network");
  };
  return {
    crateVersions: fail,
    repo: fail,
    branches: fail,
    tags: fail,
    releases: fail,
    pullRequest: fail,
    compare: fail,
  };
}

describe("network clients", () => {
  it("fetches complete crates.io version history and sends a descriptive User-Agent", async () => {
    const requests = [];
    const clients = createNetworkClients({
      fetchImpl: async (url, options) => {
        requests.push({ url: String(url), options });
        return response({ versions: [{ num: "1.0.0" }, { num: "0.9.0" }] });
      },
    });
    await expect(clients.crateVersions("demo")).resolves.toEqual(["0.9.0", "1.0.0"]);
    expect(requests).toHaveLength(1);
    expect(requests[0].options.headers["User-Agent"]).toContain("github.com");
  });

  it.each([
    ["branches", { name: "main", commit: { sha: "abc" } }],
    ["tags", { name: "v1" }],
    ["releases", { tag_name: "v1" }],
  ])("follows Link pagination for every GitHub %s collection", async (resource, item) => {
    const requests = [];
    const clients = createNetworkClients({
      githubToken: "secret-test-token",
      fetchImpl: async (url, options) => {
        requests.push({ url: String(url), options });
        return response([item], {
          responseHeaders:
            requests.length === 1 ? { link: '<https://api.github.com/next>; rel="next"' } : {},
        });
      },
    });
    await expect(clients[resource]("owner/repo")).resolves.toHaveLength(2);
    expect(requests[0].options.headers.Authorization).toBe("Bearer secret-test-token");
    expect(requests[1].url).toContain("page=2");
  });

  it("honors Retry-After before retrying a 429", async () => {
    const delays = [];
    let attempts = 0;
    const result = await fetchJson("https://example.test", {
      retries: 1,
      sleepImpl: async (delay) => delays.push(delay),
      fetchImpl: async () => {
        attempts += 1;
        return attempts === 1
          ? response({}, { status: 429, responseHeaders: { "retry-after": "3" } })
          : response({ ok: true });
      },
    });
    expect(result.data).toEqual({ ok: true });
    expect(delays).toEqual([3000]);
  });

  it("retries a GitHub-style throttling 403 when Retry-After is present", async () => {
    let attempts = 0;
    await expect(
      fetchJson("https://api.github.test", {
        retries: 1,
        sleepImpl: async () => {},
        fetchImpl: async () => {
          attempts += 1;
          return attempts === 1
            ? response({}, { status: 403, responseHeaders: { "retry-after": "0" } })
            : response({ ok: true });
        },
      }),
    ).resolves.toMatchObject({ data: { ok: true } });
    expect(attempts).toBe(2);
  });

  it("aborts a request that exceeds its timeout", async () => {
    await expect(
      fetchJson("https://example.test", {
        retries: 0,
        timeoutMs: 1,
        fetchImpl: async (_url, { signal }) =>
          new Promise((_resolve, reject) => {
            signal.addEventListener("abort", () =>
              reject(new DOMException("aborted", "AbortError")),
            );
          }),
      }),
    ).rejects.toThrow(/aborted/);
  });

  it("never exceeds configured request concurrency", async () => {
    let active = 0;
    let maximum = 0;
    const clients = createNetworkClients({
      concurrency: 2,
      fetchImpl: async () => {
        active += 1;
        maximum = Math.max(maximum, active);
        await new Promise((resolve) => setTimeout(resolve, 5));
        active -= 1;
        return response({ archived: false });
      },
    });
    await Promise.all(["a/a", "b/b", "c/c", "d/d"].map((repo) => clients.repo(repo)));
    expect(maximum).toBe(2);
  });
});

describe("observation and CLI", () => {
  it("supplies ahead/diverged ancestry evidence for every changed branch head", async () => {
    const baseline = {
      candidates: Object.fromEntries(
        TRACKED_CANDIDATES.map((name) => [name, { branches: { main: "old" } }]),
      ),
    };
    const comparisons = [];
    const observed = await observeSnapshot({
      baseline,
      now: new Date("2026-09-01T00:00:00Z"),
      clients: completeClients({
        compare: async (repo, base, head) => {
          comparisons.push({ repo, base, head });
          return { status: repo.includes("saphyr-rs") ? "diverged" : "ahead" };
        },
      }),
    });
    expect(comparisons).toHaveLength(TRACKED_CANDIDATES.length);
    expect(observed.candidates.saphyr.branches.main).toEqual({
      sha: "abc",
      relation: "diverged",
    });
    expect(observed.candidates.noyalib.branches.main).toEqual({ sha: "abc", relation: "ahead" });
  });

  it("treats an unexpected ancestry status as operational failure", async () => {
    const baseline = {
      candidates: Object.fromEntries(
        TRACKED_CANDIDATES.map((name) => [name, { branches: { main: "old" } }]),
      ),
    };
    const observed = await observeSnapshot({
      baseline,
      clients: completeClients({ compare: async () => ({ status: "unknown" }) }),
    });
    expect(observed.candidates.noyalib).toEqual({
      error: "unexpected GitHub comparison status: unknown",
    });
  });

  it("treats a malformed pending-release PR response as operational failure", async () => {
    // noyalib.pendingReleasePr is null now that PR 371 merged (#2853's release-PR
    // re-point), so observeSnapshot no longer calls clients.pullRequest for it under
    // the real config. CANDIDATE_CONFIG is only shallow-frozen, so temporarily give
    // noyalib a truthy pendingReleasePr to keep exercising this validation branch.
    const originalPendingReleasePr = CANDIDATE_CONFIG.noyalib.pendingReleasePr;
    CANDIDATE_CONFIG.noyalib.pendingReleasePr = 999;
    try {
      const observed = await observeSnapshot({
        clients: completeClients({
          pullRequest: async () => ({ state: "mystery", merged_at: null }),
        }),
      });
      expect(observed.candidates.noyalib).toEqual({ error: "malformed pull request response" });
    } finally {
      CANDIDATE_CONFIG.noyalib.pendingReleasePr = originalPendingReleasePr;
    }
  });

  it.each([
    ["network error", async () => Promise.reject(new Error("offline"))],
    ["HTTP 429", async () => response({}, { status: 429 })],
    ["HTTP 5xx", async () => response({}, { status: 503 })],
  ])("turns %s into operational failure, never drift or no-drift", async (_label, fetchImpl) => {
    const clients = createNetworkClients({ fetchImpl, retries: 0 });
    const observed = await observeSnapshot({ clients });
    const { baseline } = snapshots();
    const result = compareSnapshots(baseline, observed);
    expect(result.status).toBe(OPERATIONAL_FAILURE);
    expect(result.exitCode).not.toBe(0);
    expect(result.exitCode).not.toBe(10);
  });

  it("strips transient ancestry evidence and adds the anti-gaming comment", () => {
    const observed = {
      schemaVersion: 1,
      checkedAt: "2026-09-01T00:00:00Z",
      candidates: {
        demo: candidate({ branches: { main: { sha: "new", relation: "ahead" } } }),
      },
    };
    expect(snapshotForBaseline(observed)).toMatchObject({
      comment: BASELINE_COMMENT,
      candidates: { demo: { branches: { main: "new" } } },
    });
  });

  it("parses the locked CLI flags and rejects missing baseline paths", () => {
    expect(parseCliArgs(["--snapshot", "--json", "--baseline", "custom.json"])).toMatchObject({
      snapshot: true,
      json: true,
      baseline: "custom.json",
    });
    expect(() => parseCliArgs(["--baseline"])).toThrow(/requires a path/);
  });

  it("--snapshot writes only stdout and never modifies the baseline path", async () => {
    const directory = await mkdtemp(join(tmpdir(), "yaml-candidate-watch-"));
    const baselinePath = join(directory, "baseline.json");
    await writeFile(baselinePath, "sentinel\n");
    let stdout = "";
    let stderr = "";
    const exitCode = await runCli(["--snapshot", "--baseline", join(directory, "missing.json")], {
      clients: completeClients(),
      now: new Date("2026-09-01T00:00:00Z"),
      stdout: { write: (value) => (stdout += value) },
      stderr: { write: (value) => (stderr += value) },
    });
    expect(exitCode).toBe(0);
    expect(stderr).toBe("");
    expect(JSON.parse(stdout).comment).toBe(BASELINE_COMMENT);
    expect(await readFile(baselinePath, "utf8")).toBe("sentinel\n");
    await rm(directory, { recursive: true });
  });

  it("--render is a pure formatter: no network, and no baseline read", async () => {
    const directory = await mkdtemp(join(tmpdir(), "yaml-candidate-watch-"));
    const reportPath = join(directory, "report.json");
    const { baseline, observed } = snapshots("noyalib", informationalNoyalib());
    await writeFile(reportPath, formatJsonReport(compareSnapshots(baseline, observed)));
    let stdout = "";
    let stderr = "";
    const exitCode = await runCli(
      ["--render", reportPath, "--baseline", join(directory, "missing.json")],
      {
        clients: throwingClients(),
        stdout: { write: (value) => (stdout += value) },
        stderr: { write: (value) => (stderr += value) },
      },
    );
    expect(exitCode).toBe(0);
    expect(stderr).toBe("");
    expect(stdout).toContain("YAML candidate watch: informational-drift");
    expect(stdout).toContain("[informational] branch feat/v0.0.29 advanced 697195f -> 8888888");
    expect(() => parseCliArgs(["--render", "x", "--json"])).toThrow(/cannot be combined/);
    await rm(directory, { recursive: true });
  });

  it("exits 0 end-to-end when every observed delta is branch movement", async () => {
    const directory = await mkdtemp(join(tmpdir(), "yaml-candidate-watch-"));
    const baselinePath = join(directory, "baseline.json");
    const observedBefore = await observeSnapshot({
      clients: completeClients({
        branches: async () => [{ name: "main", commit: { sha: "old" } }],
      }),
      now: new Date("2026-09-01T00:00:00Z"),
    });
    await writeFile(baselinePath, formatJsonReport(snapshotForBaseline(observedBefore)));
    let stdout = "";
    let stderr = "";
    const exitCode = await runCli(["--json", "--baseline", baselinePath], {
      clients: completeClients(),
      now: new Date("2026-09-02T00:00:00Z"),
      stdout: { write: (value) => (stdout += value) },
      stderr: { write: (value) => (stderr += value) },
    });
    expect(exitCode).toBe(0);
    expect(stderr).toBe("");
    const parsed = JSON.parse(stdout);
    expect(parsed).toMatchObject({ status: "informational-drift", exitCode: 0, errors: [] });
    expect(
      parsed.candidates.flatMap((item) => item.deltas).filter((d) => d.kind === "branch-advanced"),
    ).toHaveLength(TRACKED_CANDIDATES.length);
    await rm(directory, { recursive: true });
  });
});

describe("committed baseline guard", () => {
  it("matches the candidate set documented in DEPENDENCIES.md", async () => {
    const baseline = JSON.parse(
      await readFile(new URL("../yaml-candidate-baseline.json", import.meta.url), "utf8"),
    );
    const dependencies = await readFile(new URL("../../DEPENDENCIES.md", import.meta.url), "utf8");
    const documented = TRACKED_CANDIDATES.filter((name) => dependencies.includes(`**\`${name}\``));
    expect(Object.keys(baseline.candidates).sort()).toEqual([...documented].sort());
    expect(documented.sort()).toEqual([...TRACKED_CANDIDATES].sort());
    expect(baseline.comment).toBe(BASELINE_COMMENT);
  });

  it("uses the canonical crate and repository mapping", async () => {
    const baseline = JSON.parse(
      await readFile(new URL("../yaml-candidate-baseline.json", import.meta.url), "utf8"),
    );
    for (const name of TRACKED_CANDIDATES) {
      expect(baseline.candidates[name]).toMatchObject({
        crate: CANDIDATE_CONFIG[name].crate,
        repo: CANDIDATE_CONFIG[name].repo,
      });
      expect(baseline.candidates[name].pendingReleasePr?.number ?? null).toBe(
        CANDIDATE_CONFIG[name].pendingReleasePr ?? null,
      );
    }
    expect(CANDIDATE_CONFIG["noyalib-serde-yaml"]).toEqual({
      crate: "noyalib-serde-yaml",
      repo: "sebastienrousseau/noyalib-serde-yaml",
      role: "adopted",
    });
  });
});
