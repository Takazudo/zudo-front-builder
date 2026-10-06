import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";
import { detectTailwindRelease } from "../detect-tailwind-release.mjs";
import { fileWindReleaseReview } from "../file-wind-release-review.mjs";

const TEST_DIR = dirname(fileURLToPath(import.meta.url));
const FIXTURE_DIR = join(TEST_DIR, "fixtures", "tailwind-release");
const WORKFLOW_PATH = join(TEST_DIR, "../..", ".github", "workflows", "tailwind-release-watch.yml");
const packumentFixture = JSON.parse(await readFile(join(FIXTURE_DIR, "packument.json"), "utf8"));
const tagFixture = JSON.parse(await readFile(join(FIXTURE_DIR, "tag-ref.json"), "utf8"));
const accepted = packumentFixture.versions["4.3.2"];
const reviewed = packumentFixture.versions["4.3.3"];
const records = {
  profile: { schemaVersion: 1, profileId: "wind-preset-free" },
  acceptedFile: {
    schemaVersion: 1,
    acceptedReference: {
      package: "tailwindcss",
      profileId: "wind-preset-free",
      version: "4.3.2",
      channel: "stable",
      integrity: accepted.dist.integrity,
    },
  },
  reviewedFile: {
    schemaVersion: 1,
    reviewedThrough: {
      package: "tailwindcss",
      version: "4.3.3",
      channel: "stable",
      integrity: reviewed.dist.integrity,
      disposition: "review-only",
    },
  },
};

function fixtureMetadata(version) {
  return {
    name: "tailwindcss",
    version,
    dist: {
      integrity: reviewed.dist.integrity,
      tarball: `https://registry.npmjs.org/tailwindcss/-/tailwindcss-${version}.tgz`,
      shasum: reviewed.dist.shasum,
    },
  };
}

function makeFixtureFetch(packument) {
  const objectSha = "4".repeat(40);
  const tag = {
    ...tagFixture,
    ref: "refs/tags/v4.4.0",
    object: {
      ...tagFixture.object,
      sha: objectSha,
      url: `https://api.github.com/repos/tailwindlabs/tailwindcss/git/commits/${objectSha}`,
    },
  };
  return async (url) => ({
    ok: true,
    status: 200,
    text: async () => JSON.stringify(url.includes("api.github.com") ? tag : packument),
  });
}

function makeFakeGh() {
  const calls = [];
  const issues = [];
  const creates = [];

  async function gh(args) {
    calls.push([...args]);
    const joined = args.join(" ");
    if (args[0] !== "api") throw new Error(`unexpected fake gh executable: ${args[0]}`);
    if (args.includes("--paginate") && joined.includes("issues?state=all&labels=")) {
      return JSON.stringify(issues);
    }
    if (joined.includes("search/issues?q=")) return JSON.stringify({ items: [] });
    if (joined.includes("labels/wind-upstream-review") && !args.includes("--method")) {
      return JSON.stringify({ name: "wind-upstream-review" });
    }
    if (joined.includes("/issues --method POST")) {
      const created = {
        number: 900,
        state: "open",
        title: args.find((arg) => arg.startsWith("title="))?.slice("title=".length),
        body: args.find((arg) => arg.startsWith("body="))?.slice("body=".length),
      };
      issues.push(created);
      creates.push(created);
      return JSON.stringify(created);
    }
    throw new Error(`unhandled fake gh call: ${joined}`);
  }

  return { calls, creates, gh };
}

async function writeAndReadReport(directory, filename, report) {
  const path = join(directory, filename);
  await writeFile(path, `${JSON.stringify(report)}\n`);
  return JSON.parse(await readFile(path, "utf8"));
}

describe("Tailwind release watch integration", () => {
  it("detects, serializes, files once, stays quiet on rerun, and no-ops an already-reviewed report", async () => {
    const packument = {
      ...packumentFixture,
      "dist-tags": { ...packumentFixture["dist-tags"], latest: "4.4.0" },
      versions: {
        ...packumentFixture.versions,
        "4.4.0": fixtureMetadata("4.4.0"),
      },
    };
    const fetchImpl = makeFixtureFetch(packument);
    const directory = await mkdtemp(join(tmpdir(), "zfb-tailwind-watch-"));
    try {
      const detected = await detectTailwindRelease({ records, fetchImpl, retries: 0 });
      expect(detected).toMatchObject({ status: "REVIEW_NEEDED", exitCode: 10 });
      expect(detected.candidate.version).toBe("4.4.0");

      const report = await writeAndReadReport(directory, "report.json", detected);
      const fake = makeFakeGh();
      const applyOptions = {
        report,
        apply: true,
        repo: "owner/repo",
        env: { GITHUB_ACTIONS: "true" },
        gh: fake.gh,
        log: () => {},
        error: (line) => {
          throw new Error(line);
        },
      };
      const first = await fileWindReleaseReview(applyOptions);
      expect(first).toMatchObject({ exitCode: 0, outcome: "created" });
      expect(fake.creates).toHaveLength(1);
      expect(fake.creates[0].body).toContain(
        "<!-- wind-release-review: tailwindcss@4.4.0 profile=wind-preset-free -->",
      );

      const rerunReport = await writeAndReadReport(directory, "report.json", detected);
      const rerun = await fileWindReleaseReview({ ...applyOptions, report: rerunReport });
      expect(rerun).toMatchObject({ exitCode: 0, outcome: "unchanged" });
      expect(fake.creates).toHaveLength(1);
      expect(
        fake.calls.filter((args) => args.includes("--method") && args.includes("POST")),
      ).toHaveLength(1);

      const alreadyReviewed = await detectTailwindRelease({
        records,
        fetchImpl: makeFixtureFetch(packumentFixture),
        retries: 0,
      });
      expect(alreadyReviewed).toMatchObject({ status: "already-reviewed", exitCode: 0 });
      expect(alreadyReviewed.candidate.source).toBeNull();
      const alreadyReviewedReport = await writeAndReadReport(
        directory,
        "already-reviewed.json",
        alreadyReviewed,
      );
      const noOpFake = makeFakeGh();
      const noOp = await fileWindReleaseReview({
        ...applyOptions,
        report: alreadyReviewedReport,
        gh: noOpFake.gh,
      });
      expect(noOp).toMatchObject({ exitCode: 0, outcome: "nothing-to-file" });
      expect(noOpFake.calls).toEqual([]);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
});

describe("Tailwind release workflow contract", () => {
  it("keeps dispatch-only, fail-closed artifact ordering, least privilege, and safe filing gates", async () => {
    const workflow = await readFile(WORKFLOW_PATH, "utf8");
    expect(workflow).not.toMatch(/^\s*schedule\s*:/m);
    expect(workflow).toContain("  workflow_dispatch:");
    expect(workflow).toContain("        type: boolean");
    expect(workflow).toContain("        default: false");
    for (const siblingCron of ["Sat 04:17", "Wed 03:43", "Mon 07:17", "Thu 05:29", "Sun 05:37"])
      expect(workflow).toContain(siblingCron);
    expect(workflow).toContain("cancel-in-progress: false");

    const uploadAt = workflow.indexOf("name: Upload detector report");
    const failClosedAt = workflow.indexOf("name: Mark a non-clean detector outcome as failed");
    expect(uploadAt).toBeGreaterThanOrEqual(0);
    expect(failClosedAt).toBeGreaterThan(uploadAt);
    expect(workflow.slice(uploadAt, failClosedAt)).toContain("if: ${{ !cancelled() }}");

    const filingAt = workflow.indexOf("  file-review-issue:");
    const trackerAt = workflow.indexOf("  track-operational-failure:");
    const filingJob = workflow.slice(filingAt, trackerAt);
    expect(filingJob).toContain("github.ref == 'refs/heads/main'");
    expect(filingJob).toContain("needs.detector.outputs.rc == '10'");
    expect(filingJob).toContain("inputs.dry_run != true");

    const detectorJob = workflow.slice(workflow.indexOf("  detector:"), filingAt);
    expect(detectorJob).not.toContain("issues: write");
    expect(detectorJob).toContain("GITHUB_TOKEN: ${{ github.token }}");
    expect(detectorJob).toContain(
      'node scripts/detect-tailwind-release.mjs detect --json > "$RUNNER_TEMP/report.json"',
    );
    expect(detectorJob).toContain("rc: ${{ steps.run_detector.outputs.rc }}");
    expect(detectorJob).toContain("status: ${{ steps.run_detector.outputs.status }}");

    const notifyAt = workflow.indexOf("  notify:");
    const trackJob = workflow.slice(trackerAt, notifyAt);
    expect(trackJob).toContain("github.ref == 'refs/heads/main'");
    expect(trackJob).toContain("inputs.dry_run != true");
    expect(trackJob).toContain("needs.file-review-issue.result == 'failure'");
    expect(trackJob).toContain("needs.detector.outputs.rc == '10'");
    expect(trackJob).toContain(
      "RUN_URL: ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}",
    );

    for (const forbidden of [
      "reference-cli.mjs promote",
      "reference-cli.mjs review",
      "pnpm install",
      "npm install",
      "gh pr merge",
    ])
      expect(workflow).not.toContain(forbidden);

    const notifyJob = workflow.slice(notifyAt);
    expect(notifyJob).toContain("needs: [detector, file-review-issue]");
    expect(notifyJob).toContain("if: ${{ !cancelled() && inputs.dry_run != true }}");
    for (const message of [
      "review needed and filed",
      "review needed but filing failed",
      "operational failure",
    ])
      expect(notifyJob).toContain(message);
    const secrets = [...workflow.matchAll(/\$\{\{\s*secrets\.([A-Z0-9_]+)\s*\}\}/g)].map(
      (match) => match[1],
    );
    expect(secrets).toEqual(["IFTTT_PROD_NOTIFY"]);
    expect(notifyJob).toContain("secrets.IFTTT_PROD_NOTIFY");
    expect(workflow.slice(0, notifyAt)).not.toMatch(/\$\{\{\s*secrets\./);

    const usesLines = [...workflow.matchAll(/^\s*uses:\s*([^\s#]+)(?:\s+#\s*(.*))?$/gm)];
    expect(usesLines.length).toBeGreaterThan(0);
    for (const [, action, version] of usesLines) {
      expect(action).toMatch(/@[0-9a-f]{40}$/);
      expect(version).toMatch(/^v\d+\.\d+\.\d+$/);
    }

    expect(workflow).toContain("Outcome matrix");
    for (const outcome of [
      "rc 0",
      "rc 10 + review issue filed",
      "rc 10 + filer failed",
      "rc 1",
      "died before writing rc",
      "dry_run=true",
      "off-main dispatch",
    ])
      expect(workflow).toContain(outcome);
  });
});
