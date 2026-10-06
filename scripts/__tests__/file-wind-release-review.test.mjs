import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

import {
  REVIEW_LABEL_DESCRIPTION,
  buildReviewBody,
  buildReviewTitle,
  fileWindReleaseReview,
} from "../file-wind-release-review.mjs";

const TEST_DIR = dirname(fileURLToPath(import.meta.url));
const FIXTURE_DIR = join(TEST_DIR, "fixtures", "tailwind-release");
const APPLY_ENV = { GITHUB_ACTIONS: "true" };

function makeReport(overrides = {}) {
  const base = {
    schemaVersion: 1,
    kind: "wind-release-detection",
    status: "REVIEW_NEEDED",
    exitCode: 10,
    package: "tailwindcss",
    profileId: "wind-preset-free",
    track: "stable",
    accepted: { version: "4.3.2", channel: "stable", integrity: "sha512-accepted" },
    reviewedThrough: {
      version: "4.3.3",
      channel: "stable",
      disposition: "review-only",
    },
    latestDistTag: "4.3.3",
    candidate: {
      version: "4.4.0",
      channel: "stable",
      integrity: "sha512-candidate",
      tarball: "https://registry.npmjs.org/tailwindcss/-/tailwindcss-4.4.0.tgz",
      sha1: "aabbccdd",
      transition: { major: false, minor: true, patch: false },
      source: {
        tag: "v4.4.0",
        observedTagObject: {
          type: "commit",
          sha: "1234567890123456789012345678901234567890",
        },
        status: "tag-observed-artifact-link-unverified",
      },
    },
    offTrackNewest: { version: "4.5.0-alpha.1", channel: "prerelease" },
    interveningVersions: ["4.3.4", "4.4.0"],
    interveningTruncated: false,
    notes: [],
    failure: null,
    identity: "sha256:report-one",
  };
  return {
    ...base,
    ...overrides,
    accepted: { ...base.accepted, ...overrides.accepted },
    reviewedThrough: { ...base.reviewedThrough, ...overrides.reviewedThrough },
    candidate:
      overrides.candidate === null
        ? null
        : {
            ...base.candidate,
            ...overrides.candidate,
            transition: { ...base.candidate.transition, ...overrides.candidate?.transition },
            source:
              overrides.candidate?.source === null
                ? null
                : { ...base.candidate.source, ...overrides.candidate?.source },
          },
  };
}

function issueFromReport(number, report, overrides = {}) {
  const body = buildReviewBody(report);
  return {
    number,
    title: buildReviewTitle(report),
    body,
    state: "open",
    ...overrides,
  };
}

function apiFailure(message, status = 500) {
  const error = new Error("gh exited with non-zero status");
  error.stderr = message;
  error.status = status;
  return error;
}

function makeFakeGh({
  listResponses = [],
  fallbackResponse = { items: [] },
  commentsByIssue = {},
  labelExists = true,
  afterCreateResponse,
} = {}) {
  const calls = [];
  const postedComments = [];
  const closedIssues = [];
  const createdIssues = [];
  let listCount = 0;
  const comments = new Map(
    Object.entries(commentsByIssue).map(([number, values]) => [Number(number), [...values]]),
  );

  async function gh(args) {
    calls.push([...args]);
    const joined = args.join(" ");
    if (args[0] !== "api") throw apiFailure(`unexpected executable: ${args[0]}`);

    if (args.includes("--paginate") && joined.includes("issues?state=all&labels=")) {
      const response = listResponses[listCount];
      listCount += 1;
      if (response !== undefined)
        return typeof response === "string" ? response : JSON.stringify(response);
      if (listCount > 1 && afterCreateResponse !== undefined) {
        return typeof afterCreateResponse === "string"
          ? afterCreateResponse
          : JSON.stringify(afterCreateResponse);
      }
      return "[]";
    }

    if (joined.includes("search/issues?q=")) {
      return typeof fallbackResponse === "string"
        ? fallbackResponse
        : JSON.stringify(fallbackResponse);
    }

    if (args.includes("--paginate") && /issues\/\d+\/comments\?per_page=100/.test(joined)) {
      const match = /issues\/(\d+)\/comments/.exec(joined);
      const rows = comments.get(Number(match[1])) ?? [];
      if (rows.length === 1 && typeof rows[0] === "string") return rows[0];
      return JSON.stringify(rows);
    }

    if (joined.includes("labels/wind-upstream-review") && !args.includes("--method")) {
      if (!labelExists) throw apiFailure("HTTP 404 Not Found", 404);
      return JSON.stringify({ name: "wind-upstream-review" });
    }

    if (joined.includes("/labels --method POST")) {
      return JSON.stringify({ name: "wind-upstream-review" });
    }

    if (joined.includes("/issues --method POST")) {
      const titleArg = args.find((arg) => arg.startsWith("title="));
      const bodyArg = args.find((arg) => arg.startsWith("body="));
      const created = {
        number: 900,
        state: "open",
        title: titleArg?.slice("title=".length),
        body: bodyArg?.slice("body=".length),
      };
      createdIssues.push(created);
      return JSON.stringify(created);
    }

    const commentPost = /issues\/(\d+)\/comments/.exec(joined);
    if (commentPost && args.includes("--method") && args.includes("POST")) {
      const number = Number(commentPost[1]);
      const bodyArg = args.find((arg) => arg.startsWith("body="));
      const comment = { body: bodyArg?.slice("body=".length) ?? "" };
      postedComments.push({ number, body: comment.body });
      comments.set(number, [...(comments.get(number) ?? []), comment]);
      return JSON.stringify({ id: postedComments.length, ...comment });
    }

    const closeMatch = /issues\/(\d+)/.exec(joined);
    if (closeMatch && args.includes("--method") && args.includes("PATCH")) {
      closedIssues.push(Number(closeMatch[1]));
      return JSON.stringify({ number: Number(closeMatch[1]), state: "closed" });
    }

    throw apiFailure(`unhandled fake gh call: ${joined}`);
  }

  return { gh, calls, postedComments, closedIssues, createdIssues, comments };
}

async function run(report, fake, options = {}) {
  const output = [];
  const errors = [];
  const result = await fileWindReleaseReview({
    report,
    apply: true,
    repo: "owner/repo",
    env: APPLY_ENV,
    gh: fake.gh,
    log: (line) => output.push(line),
    error: (line) => errors.push(line),
    ...options,
  });
  return { ...result, output, errors };
}

describe("report validation and dry-run contract", () => {
  it.each([
    ["kind", { kind: "other" }],
    ["schemaVersion", { schemaVersion: 2 }],
    ["status", { status: "unknown", exitCode: 0 }],
    ["status exitCode", { exitCode: 0 }],
    ["missing candidate", { candidate: null }],
  ])("rejects a malformed %s before calling gh", async (_name, overrides) => {
    const fake = makeFakeGh();
    const result = await run(makeReport(overrides), fake);
    expect(result.exitCode).toBe(1);
    expect(result.errors[0]).toMatch(/^malformed report:/);
    expect(fake.calls).toEqual([]);
  });

  it("reports non-review statuses as a no-op before any gh call", async () => {
    const fake = makeFakeGh();
    const report = makeReport({ status: "prerelease-only", exitCode: 0, candidate: null });
    const result = await run(report, fake);
    expect(result.exitCode).toBe(0);
    expect(result.output).toContain("nothing to file (prerelease-only)");
    expect(fake.calls).toEqual([]);
  });

  it("prints the complete report-derived dry-run plan without calling gh", async () => {
    const fake = makeFakeGh();
    const output = [];
    const result = await fileWindReleaseReview({
      report: makeReport(),
      repo: "owner/repo",
      gh: fake.gh,
      log: (line) => output.push(line),
    });
    const printed = output.join("\n");
    expect(result).toMatchObject({ exitCode: 0, outcome: "dry-run" });
    expect(printed).toContain("Title: [wind-upstream] tailwindcss 4.4.0 (stable) needs review");
    expect(printed).toContain(
      "<!-- wind-release-review: tailwindcss@4.4.0 profile=wind-preset-free -->",
    );
    expect(printed).toContain("<!-- wind-release-review-identity: sha256:report-one -->");
    expect(printed).toContain('"candidate":"4.4.0"');
    expect(printed).toContain(
      "would list existing `wind-upstream-review` issues; would create or comment accordingly",
    );
    expect(printed).toContain("Conditional gh calls for this report:");
    expect(printed).toContain("4.4.0");
    expect(fake.calls).toEqual([]);
  });

  it("refuses local apply without the CI flag or explicit test escape hatch", async () => {
    const fake = makeFakeGh();
    const errors = [];
    const result = await fileWindReleaseReview({
      report: makeReport(),
      apply: true,
      repo: "owner/repo",
      env: {},
      gh: fake.gh,
      log: () => {},
      error: (line) => errors.push(line),
    });
    expect(result).toMatchObject({ exitCode: 1, outcome: "apply-blocked" });
    expect(errors.join("\n")).toContain("apply is CI-only");
    expect(fake.calls).toEqual([]);
  });
});

describe("review issue content", () => {
  it("includes every report field, major transition warning, manual command, and related reference", () => {
    const report = makeReport({
      candidate: {
        version: "5.0.0",
        transition: { major: true, minor: false, patch: false },
        source: {
          observedTagObject: { type: "tag", sha: "abcdef0123456789abcdef0123456789abcdef01" },
        },
      },
      interveningTruncated: true,
    });
    const body = buildReviewBody(report);
    expect(buildReviewTitle(report)).toBe(
      "[wind-upstream] tailwindcss 5.0.0 (stable) needs review",
    );
    expect(body).toContain("tailwindcss@4.3.2");
    expect(body).toContain("tailwindcss@4.3.3");
    expect(body).toContain("disposition: `review-only`");
    expect(body).toContain("SRI integrity: `sha512-candidate`");
    expect(body).toContain("tailwindcss-4.4.0.tgz");
    expect(body).toContain("SHA-1: `aabbccdd`");
    expect(body).toContain("MAJOR VERSION TRANSITION");
    expect(body).toContain("type `tag`, SHA `abcdef0123456789abcdef0123456789abcdef01`");
    expect(body).toContain("The list is truncated after 30 versions.");
    expect(body).toContain("4.5.0-alpha.1");
    expect(body).toContain("--candidate 4.3.2 --channel stable");
    expect(body).toContain("--candidate 5.0.0 --channel stable");
    expect(body).not.toContain("--candidate 4.3.3 --channel stable");
    expect(body).toContain(
      'EVIDENCE="$(mktemp -d "${TMPDIR:-/tmp}/wind-manual-following.XXXXXX")"',
    );
    expect(body).toContain("https://www.npmjs.com/package/tailwindcss/v/5.0.0");
    expect(body).toContain("https://github.com/tailwindlabs/tailwindcss/releases/tag/v5.0.0");
    for (const number of [3813, 3821, 3827, 3822]) expect(body).toContain(`#${number}`);
    expect(body).toContain("tests/wind-compatibility/reference/README.md");
    expect(body).toContain("tests/wind-compatibility/reference/manual-following.md");
    expect(body).toContain(
      "Detection is not adoption or compatibility verification. No assessment or comparison has run. The tag-to-npm-artifact link is unverified.",
    );
    expect(body).not.toMatch(/\/Users\/|\/home\/|RUNNER_TEMP/);
  });

  it("renders the missing upstream tag explicitly", () => {
    const body = buildReviewBody(
      makeReport({ candidate: { source: { status: "tag-missing", observedTagObject: null } } }),
    );
    expect(body).toContain("Tag observation: `tag-missing`.");
    expect(body).toContain('"tag":null');
  });
});

describe("GitHub issue filing", () => {
  it("creates a labeled issue with both markers and the machine-readable data line", async () => {
    const emptyIssuesFixture = JSON.parse(
      await readFile(join(FIXTURE_DIR, "issues-page-1.json"), "utf8"),
    );
    const fake = makeFakeGh({
      listResponses: [emptyIssuesFixture, [issueFromReport(900, makeReport())]],
      labelExists: false,
    });
    const result = await run(makeReport(), fake);
    expect(result).toMatchObject({ exitCode: 0, outcome: "created" });
    expect(fake.calls.filter((args) => args.includes("--paginate"))).toHaveLength(2);
    expect(fake.calls.some((args) => args.includes("name=wind-upstream-review"))).toBe(true);
    expect(fake.createdIssues).toHaveLength(1);
    const created = fake.createdIssues[0];
    expect(created.title).toBe("[wind-upstream] tailwindcss 4.4.0 (stable) needs review");
    expect(created.body).toContain(
      "<!-- wind-release-review: tailwindcss@4.4.0 profile=wind-preset-free -->",
    );
    expect(created.body).toContain("<!-- wind-release-review-identity: sha256:report-one -->");
    expect(created.body).toContain(
      '<!-- wind-release-review-data: {"accepted":"4.3.2","reviewedThrough":"4.3.3","candidate":"4.4.0","integrity":"sha512-candidate","tarball":"https://registry.npmjs.org/tailwindcss/-/tailwindcss-4.4.0.tgz","tag":"1234567890123456789012345678901234567890"} -->',
    );
    expect(created.body).toContain("## Reference records");
    expect(created.body).toContain("## Candidate release");
    expect(created.body).toContain("## Upstream references");
    expect(created.body).toContain("## Manual assessment command");
    expect(created.body).toContain("## Related references");
    expect(created.body).toContain("--candidate 4.4.0 --channel stable");
    expect(created.body).not.toMatch(/\/Users\/|\/home\/|RUNNER_TEMP/);
    expect(REVIEW_LABEL_DESCRIPTION.length).toBeLessThanOrEqual(100);
  });

  it("does not treat a pull request body marker as a matching issue", async () => {
    const fake = makeFakeGh({
      listResponses: [
        [
          {
            ...issueFromReport(41, makeReport()),
            pull_request: { url: "https://api.github.com/pulls/41" },
          },
        ],
        [issueFromReport(900, makeReport())],
      ],
    });
    const result = await run(makeReport(), fake);
    expect(result.outcome).toBe("created");
    expect(fake.createdIssues).toHaveLength(1);
    expect(fake.calls.some((args) => args.join(" ").includes("search/issues?q="))).toBe(true);
  });

  it("quietly recognizes an open issue by its body marker even when the title changed", async () => {
    const report = makeReport();
    const fake = makeFakeGh({
      listResponses: [[issueFromReport(51, report, { title: "Human-edited title" })]],
    });
    const result = await run(report, fake);
    expect(result).toMatchObject({ exitCode: 0, outcome: "unchanged" });
    expect(fake.postedComments).toEqual([]);
  });

  it("uses a report identity in a comment to make a changed report idempotent", async () => {
    const oldReport = makeReport();
    const newReport = makeReport({
      identity: "sha256:report-two",
      candidate: {
        integrity: "sha512-candidate-updated",
        tarball: "https://registry.npmjs.org/tailwindcss/-/tailwindcss-4.4.0-new.tgz",
        source: {
          observedTagObject: { type: "commit", sha: "abcdefabcdefabcdefabcdefabcdefabcdefabcd" },
        },
      },
    });
    const realComments = JSON.parse(
      await readFile(join(FIXTURE_DIR, "comments-page-3896.json"), "utf8"),
    );
    const fake = makeFakeGh({
      listResponses: [[issueFromReport(62, oldReport)], [issueFromReport(62, oldReport)]],
      commentsByIssue: { 62: realComments },
    });
    const first = await run(newReport, fake);
    const second = await run(newReport, fake);
    expect(first).toMatchObject({ exitCode: 0, outcome: "commented" });
    expect(fake.postedComments).toHaveLength(1);
    expect(fake.postedComments[0].number).toBe(62);
    expect(fake.postedComments[0].body).toContain(
      "<!-- wind-release-review-identity: sha256:report-two -->",
    );
    expect(fake.postedComments[0].body).toContain(
      "SRI integrity: `sha512-candidate` → `sha512-candidate-updated`",
    );
    expect(fake.postedComments[0].body).toContain("Tarball URL:");
    expect(fake.postedComments[0].body).toContain("Observed tag object SHA:");
    expect(second).toMatchObject({ exitCode: 0, outcome: "unchanged" });
    expect(fake.postedComments).toHaveLength(1);
  });

  it("quietly honors an identity marker in the issue body before comparing its data line", async () => {
    const oldReport = makeReport();
    const newReport = makeReport({
      candidate: { tarball: "https://registry.npmjs.org/tailwindcss/-/changed.tgz" },
    });
    const fake = makeFakeGh({ listResponses: [[issueFromReport(63, oldReport)]] });
    const result = await run(newReport, fake);
    expect(result.outcome).toBe("unchanged");
    expect(fake.postedComments).toEqual([]);
    expect(fake.calls).toHaveLength(1);
  });

  it("suppresses a duplicate report identity already present in a paginated comment result", async () => {
    const oldReport = makeReport();
    const report = makeReport({
      identity: "sha256:report-two",
      candidate: { integrity: "sha512-candidate-updated" },
    });
    const issue = issueFromReport(64, oldReport);
    const commentPage1 = [{ body: "human comment without an identity" }];
    const commentPage2 = [
      { body: `automated\n<!-- wind-release-review-identity: ${report.identity} -->` },
    ];
    const fake = makeFakeGh({
      listResponses: [[issue]],
      commentsByIssue: { 64: [`${JSON.stringify(commentPage1)}\n${JSON.stringify(commentPage2)}`] },
    });
    const result = await run(report, fake);
    expect(result.outcome).toBe("unchanged");
    expect(fake.postedComments).toEqual([]);
  });

  it("prefers an open match over a closed match", async () => {
    const report = makeReport();
    const closed = issueFromReport(70, report, { state: "closed" });
    const open = issueFromReport(71, report);
    const fake = makeFakeGh({ listResponses: [[closed, open]] });
    const result = await run(report, fake);
    expect(result.outcome).toBe("unchanged");
    expect(result.output).not.toContain("already-reviewed-by-human #70");
  });

  it("treats a closed marker match as reviewed by a human", async () => {
    const report = makeReport();
    const fake = makeFakeGh({
      listResponses: [
        [issueFromReport(73, report, { title: "A title edited by a reviewer", state: "closed" })],
      ],
    });
    const result = await run(report, fake);
    expect(result).toMatchObject({ exitCode: 0, outcome: "already-reviewed-by-human" });
    expect(result.output).toContain("already-reviewed-by-human #73");
    expect(fake.calls).toHaveLength(1);
  });

  it("comments on and closes every higher-numbered open duplicate", async () => {
    const report = makeReport();
    const fake = makeFakeGh({
      listResponses: [
        [issueFromReport(80, report), issueFromReport(74, report), issueFromReport(88, report)],
      ],
    });
    const result = await run(report, fake);
    expect(result.outcome).toBe("unchanged");
    expect(fake.postedComments.map(({ number, body }) => ({ number, body }))).toEqual([
      { number: 80, body: expect.stringContaining("duplicate of #74") },
      { number: 88, body: expect.stringContaining("duplicate of #74") },
    ]);
    expect(fake.closedIssues).toEqual([80, 88]);
  });

  it("reconciles duplicates discovered in the one post-create listing", async () => {
    const report = makeReport();
    const newIssue = issueFromReport(90, report);
    const duplicate = issueFromReport(93, report);
    const fake = makeFakeGh({
      listResponses: [[], [newIssue, duplicate]],
    });
    const result = await run(report, fake);
    expect(result.outcome).toBe("created");
    expect(fake.createdIssues).toHaveLength(1);
    expect(fake.postedComments).toHaveLength(1);
    expect(fake.postedComments[0].number).toBe(93);
    expect(fake.postedComments[0].body).toContain("duplicate of #90");
    expect(fake.closedIssues).toEqual([93]);
  });

  it("reads marker matches across fully paginated issue-list output", async () => {
    const report = makeReport();
    const unmatched = { number: 101, body: "unrelated body", state: "open" };
    const match = issueFromReport(103, report, { title: "renamed" });
    const fake = makeFakeGh({
      listResponses: [[[unmatched], [match]].map((page) => JSON.stringify(page)).join("\n")],
    });
    const result = await run(report, fake);
    expect(result).toMatchObject({ exitCode: 0, outcome: "unchanged" });
    expect(
      fake.calls.filter((args) => args.at(-1).includes("issues?state=all&labels=")),
    ).toHaveLength(1);
    expect(fake.calls.some((args) => args.join(" ").includes("search/issues?q="))).toBe(false);
  });

  it("returns gh stderr and exits 1 when a GitHub request fails", async () => {
    const errors = [];
    const fake = makeFakeGh();
    fake.gh = async (args) => {
      fake.calls.push([...args]);
      throw apiFailure("gh: API rate limit exceeded", 403);
    };
    const result = await run(makeReport(), fake, { error: (line) => errors.push(line) });
    expect(result).toMatchObject({ exitCode: 1, outcome: "gh-failure" });
    expect(errors).toEqual(["gh: API rate limit exceeded"]);
  });
});
