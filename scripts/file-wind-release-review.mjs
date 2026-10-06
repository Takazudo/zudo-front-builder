#!/usr/bin/env node

import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

export const REPORT_KIND = "wind-release-detection";
export const REPORT_SCHEMA_VERSION = 1;
export const REVIEW_LABEL = "wind-upstream-review";
export const REVIEW_LABEL_COLOR = "1D76DB";
export const REVIEW_LABEL_DESCRIPTION =
  "Tailwind upstream release requiring manual compatibility review";

const EXIT_CODE_BY_STATUS = Object.freeze({
  "no-change": 0,
  "already-reviewed": 0,
  "prerelease-only": 0,
  REVIEW_NEEDED: 10,
  "operational-failure": 1,
});

const DATA_FIELDS = Object.freeze([
  "accepted",
  "reviewedThrough",
  "candidate",
  "integrity",
  "tarball",
  "tag",
]);

const FIELD_LABELS = Object.freeze({
  accepted: "Accepted version",
  reviewedThrough: "Reviewed-through version",
  candidate: "Candidate version",
  integrity: "SRI integrity",
  tarball: "Tarball URL",
  tag: "Observed tag object SHA",
});

const MANUAL_CASES = "block,hidden,p-0,mx-auto,contents";
const PROJECT = "Takazudo/zudo-front-builder";

async function runGh(args) {
  const { stdout } = await execFileAsync("gh", args, { encoding: "utf8" });
  return stdout;
}

function reportError(message) {
  const error = new Error(message);
  error.name = "MalformedReportError";
  return error;
}

/** Reject unsupported reports before any GitHub request can be made. */
export function validateReport(report) {
  if (!report || typeof report !== "object" || Array.isArray(report)) {
    throw reportError("expected a JSON object");
  }
  if (report.kind !== REPORT_KIND) throw reportError(`unknown kind ${JSON.stringify(report.kind)}`);
  if (report.schemaVersion !== REPORT_SCHEMA_VERSION) {
    throw reportError(`unknown schemaVersion ${JSON.stringify(report.schemaVersion)}`);
  }
  if (!Object.hasOwn(EXIT_CODE_BY_STATUS, report.status)) {
    throw reportError(`unknown status ${JSON.stringify(report.status)}`);
  }
  if (report.exitCode !== EXIT_CODE_BY_STATUS[report.status]) {
    throw reportError(`exitCode does not match status ${report.status}`);
  }
  if (report.status === "REVIEW_NEEDED") {
    if (
      !report.candidate ||
      typeof report.candidate !== "object" ||
      Array.isArray(report.candidate)
    ) {
      throw reportError("REVIEW_NEEDED has no candidate");
    }
    const requiredStrings = [
      ["identity", report.identity],
      ["profileId", report.profileId],
      ["accepted.version", report.accepted?.version],
      ["accepted.channel", report.accepted?.channel],
      ["reviewedThrough.version", report.reviewedThrough?.version],
      ["reviewedThrough.channel", report.reviewedThrough?.channel],
      ["candidate.version", report.candidate.version],
      ["candidate.channel", report.candidate.channel],
    ];
    for (const [name, value] of requiredStrings) {
      if (typeof value !== "string" || value.length === 0) {
        throw reportError(`missing ${name}`);
      }
    }
    if (/[\r\n<>]/.test(report.identity)) throw reportError("invalid identity marker value");
    if (/[\r\n<>]/.test(report.profileId)) throw reportError("invalid profileId marker value");
  }
  return report;
}

function dataForReport(report) {
  const observedTagObject = report.candidate?.source?.observedTagObject;
  return {
    accepted: report.accepted?.version ?? null,
    reviewedThrough: report.reviewedThrough?.version ?? null,
    candidate: report.candidate?.version ?? null,
    integrity: report.candidate?.integrity ?? null,
    tarball: report.candidate?.tarball ?? null,
    tag: observedTagObject?.sha ?? null,
  };
}

function issueMarker(report) {
  return `<!-- wind-release-review: tailwindcss@${report.candidate.version} profile=${report.profileId} -->`;
}

function reportIdentityMarker(report) {
  return `<!-- wind-release-review-identity: ${report.identity} -->`;
}

function dataMarker(data) {
  return `<!-- wind-release-review-data: ${JSON.stringify(data)} -->`;
}

function markdownValue(value) {
  if (value === null || value === undefined || value === "") return "(missing)";
  return String(value).replaceAll("`", "\\`").replaceAll("\n", " ").replaceAll("\r", " ");
}

function transitionSummary(transition = {}) {
  const flags = ["major", "minor", "patch"].map(
    (key) => `${key}=${transition[key] === true ? "yes" : "no"}`,
  );
  if (transition.major === true) {
    return `**MAJOR VERSION TRANSITION: ${flags.join(", ")}** — manual review must account for the major-version transition.`;
  }
  return `Transition from accepted: ${flags.join(", ")}.`;
}

function tagSummary(source) {
  if (source?.status === "tag-missing") return "Tag observation: `tag-missing`.";
  const object = source?.observedTagObject;
  if (object && typeof object.type === "string" && typeof object.sha === "string") {
    const status = source.status ? `; source status: \`${source.status}\`` : "";
    return `Observed tag object: type \`${object.type}\`, SHA \`${object.sha}\`${status}.`;
  }
  return `Tag observation: \`${source?.status ?? "unavailable"}\`. No tag object was recorded.`;
}

function versionList(report) {
  const versions = Array.isArray(report.interveningVersions) ? report.interveningVersions : [];
  const rendered =
    versions.length > 0 ? versions.map((version) => `\`${version}\``).join(", ") : "none";
  const truncated =
    report.interveningTruncated === true ? " The list is truncated after 30 versions." : "";
  return `${rendered}.${truncated}`;
}

function offTrackSummary(offTrackNewest) {
  if (!offTrackNewest) return "None.";
  return `\`${offTrackNewest.version}\` on channel \`${offTrackNewest.channel}\`.`;
}

function assessmentCommand(report) {
  const accepted = report.accepted;
  const candidate = report.candidate;
  return [
    'EVIDENCE="$(mktemp -d "${TMPDIR:-/tmp}/wind-manual-following.XXXXXX")"',
    'CACHE="$EVIDENCE/cache"',
    'mkdir -p "$CACHE"',
    "",
    "node scripts/wind-compatibility/reference-cli.mjs acquire \\",
    `  --candidate ${accepted.version} --channel ${accepted.channel} --live yes --cache "$CACHE" \\`,
    '  > "$EVIDENCE/accepted-acquisition.json"',
    "node scripts/wind-compatibility/reference-cli.mjs probe \\",
    `  --candidate ${accepted.version} --channel ${accepted.channel} --live yes --cache "$CACHE" \\`,
    `  --candidates ${MANUAL_CASES} \\`,
    '  > "$EVIDENCE/accepted-probe.json"',
    "node scripts/wind-compatibility/reference-cli.mjs plan \\",
    `  --candidate ${candidate.version} --channel ${candidate.channel} --live yes > "$EVIDENCE/plan.json"`,
    "node scripts/wind-compatibility/reference-cli.mjs acquire \\",
    `  --candidate ${candidate.version} --channel ${candidate.channel} --live yes --cache "$CACHE" \\`,
    '  > "$EVIDENCE/acquisition.json"',
    "node scripts/wind-compatibility/reference-cli.mjs probe \\",
    `  --candidate ${candidate.version} --channel ${candidate.channel} --live yes --cache "$CACHE" \\`,
    `  --candidates ${MANUAL_CASES} \\`,
    '  > "$EVIDENCE/candidate-probe.json"',
    "node scripts/wind-compatibility/reference-cli.mjs assess \\",
    '  --plan "$EVIDENCE/plan.json" --cache "$CACHE" \\',
    '  > "$EVIDENCE/assessment.json"',
  ].join("\n");
}

/** Build the complete human-readable issue body from the detection report. */
export function buildReviewBody(report) {
  const data = dataForReport(report);
  const source = report.candidate.source;
  const acceptedDisposition = report.accepted.disposition ?? "accepted";
  const reviewedDisposition = report.reviewedThrough.disposition ?? "unspecified";
  const upstreamVersion = encodeURIComponent(report.candidate.version);
  const upstreamReleaseVersion = encodeURIComponent(`v${report.candidate.version}`);

  return [
    issueMarker(report),
    reportIdentityMarker(report),
    dataMarker(data),
    "",
    "## Reference records",
    `- Accepted: \`tailwindcss@${report.accepted.version}\` on channel \`${report.accepted.channel}\`; disposition: \`${acceptedDisposition}\`.`,
    `- Reviewed through: \`tailwindcss@${report.reviewedThrough.version}\` on channel \`${report.reviewedThrough.channel}\`; disposition: \`${reviewedDisposition}\`.`,
    "",
    "## Candidate release",
    `- Version: \`tailwindcss@${report.candidate.version}\` on channel \`${report.candidate.channel}\`.`,
    `- SRI integrity: \`${report.candidate.integrity ?? "(missing)"}\`.`,
    `- Tarball: ${report.candidate.tarball ?? "(missing)"}`,
    `- SHA-1: \`${report.candidate.sha1 ?? "(missing)"}\`.`,
    `- ${transitionSummary(report.candidate.transition)}`,
    `- ${tagSummary(source)}`,
    `- Intervening on-track versions from accepted through candidate: ${versionList(report)}`,
    `- Newest off-track version above accepted: ${offTrackSummary(report.offTrackNewest)}`,
    "",
    "## Upstream references",
    `- [npm package version](https://www.npmjs.com/package/tailwindcss/v/${upstreamVersion})`,
    `- [Tailwind CSS release](https://github.com/tailwindlabs/tailwindcss/releases/tag/${upstreamReleaseVersion})`,
    "",
    "## Manual assessment command",
    "Run this only after a reviewer chooses to assess the candidate. It keeps the cache and evidence outside the checkout.",
    "",
    "```sh",
    assessmentCommand(report),
    "```",
    "",
    "## Related references",
    ...[3813, 3821, 3827, 3822].map(
      (number) => `- [#${number}](https://github.com/${PROJECT}/issues/${number})`,
    ),
    `- [Wind reference README](https://github.com/${PROJECT}/blob/main/tests/wind-compatibility/reference/README.md)`,
    `- [Manual following procedure](https://github.com/${PROJECT}/blob/main/tests/wind-compatibility/reference/manual-following.md)`,
    "",
    "Detection is not adoption or compatibility verification. No assessment or comparison has run. The tag-to-npm-artifact link is unverified.",
  ].join("\n");
}

export function buildReviewTitle(report) {
  return `[wind-upstream] tailwindcss ${report.candidate.version} (${report.candidate.channel}) needs review`;
}

function dataFromIssueBody(body) {
  const match = /<!-- wind-release-review-data: (.*?) -->/.exec(String(body ?? ""));
  if (!match) return null;
  try {
    const value = JSON.parse(match[1]);
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    return value;
  } catch {
    return null;
  }
}

function markerAlreadyRecorded(text, marker) {
  return String(text ?? "").includes(marker);
}

function changedFields(previous, current) {
  if (!previous) return [...DATA_FIELDS];
  return DATA_FIELDS.filter((field) => (previous[field] ?? null) !== (current[field] ?? null));
}

function buildDeltaComment(previousData, report) {
  const currentData = dataForReport(report);
  const changed = changedFields(previousData, currentData);
  const rows = changed.map((field) => {
    const oldValue = previousData?.[field] ?? null;
    const newValue = currentData[field] ?? null;
    return `- ${FIELD_LABELS[field]}: \`${markdownValue(oldValue)}\` → \`${markdownValue(newValue)}\``;
  });
  return [
    reportIdentityMarker(report),
    "Tailwind release detection changed the release review details:",
    ...rows,
  ].join("\n");
}

function parseJsonDocuments(stdout) {
  const text = String(stdout ?? "").trim();
  if (!text) return [];
  const documents = [];
  let start = -1;
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (start < 0) {
      if (/\s/.test(char)) continue;
      if (char !== "[" && char !== "{") throw new Error("gh returned invalid JSON");
      start = index;
      depth = 1;
      continue;
    }
    if (inString) {
      if (escaped) escaped = false;
      else if (char === "\\") escaped = true;
      else if (char === '"') inString = false;
      continue;
    }
    if (char === '"') {
      inString = true;
      continue;
    }
    if (char === "[" || char === "{") depth += 1;
    else if (char === "]" || char === "}") {
      depth -= 1;
      if (depth === 0) {
        documents.push(JSON.parse(text.slice(start, index + 1)));
        start = -1;
      }
    }
  }
  if (start >= 0 || inString) throw new Error("gh returned incomplete JSON");
  return documents;
}

function listItems(stdout) {
  return parseJsonDocuments(stdout).flatMap((document) => {
    if (Array.isArray(document)) return document;
    if (Array.isArray(document?.items)) return document.items;
    return [];
  });
}

function parseObject(stdout) {
  const documents = parseJsonDocuments(stdout);
  return (
    documents.find(
      (document) => document && typeof document === "object" && !Array.isArray(document),
    ) ?? null
  );
}

function isNotFound(error) {
  return (
    error?.status === 404 ||
    error?.statusCode === 404 ||
    /\b404\b|not found/i.test(errorText(error))
  );
}

function errorText(error) {
  const stderr = typeof error?.stderr === "string" ? error.stderr.trim() : "";
  return stderr || error?.message || String(error);
}

function validateRepo(repo) {
  if (typeof repo !== "string" || !/^[^/\s]+\/[^/\s]+$/.test(repo)) {
    throw new Error("GITHUB_REPOSITORY must be owner/repository");
  }
  return repo;
}

function endpoint(repo, suffix = "") {
  return `repos/${repo}/${suffix}`.replace(/\/$/, "");
}

function issueIsPullRequest(issue) {
  return Object.hasOwn(issue ?? {}, "pull_request");
}

function matchesIssue(issue, marker) {
  return Boolean(issue && !issueIsPullRequest(issue) && String(issue.body ?? "").includes(marker));
}

function matchingIssues(issues, marker) {
  return issues
    .filter((issue) => matchesIssue(issue, marker))
    .sort((left, right) => Number(left.number) - Number(right.number));
}

function pullRequestIdentitySearch(repo, report) {
  const plainMarker = `wind-release-review: tailwindcss@${report.candidate.version}`;
  return `search/issues?q=repo:${repo}+is:issue+"${plainMarker}"+in:body`;
}

async function listReviewIssues(gh, repo) {
  const apiPath = endpoint(
    repo,
    `issues?state=all&labels=${encodeURIComponent(REVIEW_LABEL)}&per_page=100`,
  );
  const stdout = await gh(["api", "--paginate", apiPath]);
  return listItems(stdout);
}

async function listComments(gh, repo, issueNumber) {
  const apiPath = endpoint(repo, `issues/${issueNumber}/comments?per_page=100`);
  const stdout = await gh(["api", "--paginate", apiPath]);
  return listItems(stdout);
}

async function findMatchingIssues(gh, repo, report, marker) {
  const listed = matchingIssues(await listReviewIssues(gh, repo), marker);
  if (listed.length > 0) return listed;
  const fallbackPath = pullRequestIdentitySearch(repo, report);
  const fallback = await gh(["api", fallbackPath]);
  const searchResult = parseObject(fallback);
  return matchingIssues(Array.isArray(searchResult?.items) ? searchResult.items : [], marker);
}

async function ensureReviewLabel(gh, repo) {
  const labelPath = endpoint(repo, `labels/${encodeURIComponent(REVIEW_LABEL)}`);
  try {
    await gh(["api", labelPath]);
    return;
  } catch (error) {
    if (!isNotFound(error)) throw error;
  }
  await gh([
    "api",
    endpoint(repo, "labels"),
    "--method",
    "POST",
    "-f",
    `name=${REVIEW_LABEL}`,
    "-f",
    `color=${REVIEW_LABEL_COLOR}`,
    "-f",
    `description=${REVIEW_LABEL_DESCRIPTION}`,
  ]);
}

async function postComment(gh, repo, issueNumber, body) {
  await gh([
    "api",
    endpoint(repo, `issues/${issueNumber}/comments`),
    "--method",
    "POST",
    "-f",
    `body=${body}`,
  ]);
}

async function reconcileDuplicates(gh, repo, issues) {
  const open = issues.filter((issue) => String(issue.state).toLowerCase() === "open");
  if (open.length < 2) return;
  open.sort((left, right) => Number(left.number) - Number(right.number));
  const canonical = Number(open[0].number);
  for (const duplicate of open.slice(1)) {
    const number = Number(duplicate.number);
    await postComment(
      gh,
      repo,
      number,
      `This issue is a duplicate of #${canonical}; closing it after review-issue deduplication.`,
    );
    await gh([
      "api",
      endpoint(repo, `issues/${number}`),
      "--method",
      "PATCH",
      "-f",
      "state=closed",
    ]);
  }
}

async function refreshOpenIssue(gh, repo, issue, report) {
  const identityMarker = reportIdentityMarker(report);
  if (markerAlreadyRecorded(issue.body, identityMarker)) return "unchanged";
  const comments = await listComments(gh, repo, issue.number);
  if (comments.some((comment) => markerAlreadyRecorded(comment.body, identityMarker)))
    return "unchanged";

  const previousData = dataFromIssueBody(issue.body);
  const currentData = dataForReport(report);
  const changed = changedFields(previousData, currentData);
  if (changed.length === 0) return "unchanged";
  await postComment(gh, repo, issue.number, buildDeltaComment(previousData, report));
  return "commented";
}

function quotePlan(value) {
  return `'${String(value).replaceAll("'", `'\\''`)}'`;
}

function printDryRunPlan(report, repo, log) {
  const title = buildReviewTitle(report);
  const body = buildReviewBody(report);
  const marker = issueMarker(report);
  const identity = reportIdentityMarker(report);
  const data = dataMarker(dataForReport(report));
  const issueList = endpoint(
    repo,
    `issues?state=all&labels=${encodeURIComponent(REVIEW_LABEL)}&per_page=100`,
  );
  log("DRY RUN: no gh commands were called.");
  log(`Title: ${title}`);
  log(`Identity marker: ${marker}`);
  log(`Report identity marker: ${identity}`);
  log(`Report data marker: ${data}`);
  log("Body:");
  log(body);
  log(`would list existing \`${REVIEW_LABEL}\` issues; would create or comment accordingly`);
  log("Conditional gh calls for this report:");
  log(`- gh api --paginate ${quotePlan(issueList)}`);
  log(
    `- gh api ${quotePlan(`search/issues?q=repo:${repo}+is:issue+"wind-release-review: tailwindcss@${report.candidate.version}"+in:body`)}`,
  );
  log(
    `- gh api ${quotePlan(endpoint(repo, `labels/${REVIEW_LABEL}`))} (check; create label on 404)`,
  );
  log(
    `- gh api ${quotePlan(endpoint(repo, "labels"))} --method POST -f name=${REVIEW_LABEL} -f color=${REVIEW_LABEL_COLOR} -f description=${quotePlan(REVIEW_LABEL_DESCRIPTION)}`,
  );
  log(`- gh api ${quotePlan(endpoint(repo, "issues"))} --method POST (create review issue)`);
  log(
    `- gh api ${quotePlan(endpoint(repo, "issues/<number>/comments"))} --method POST (comment or close duplicates)`,
  );
  log(
    `- gh api ${quotePlan(endpoint(repo, "issues/<number>"))} --method PATCH -f state=closed (close duplicates)`,
  );
}

/**
 * File or refresh a Tailwind release review issue.
 *
 * @param {object} options
 * @param {object} options.report Detection report from detect-tailwind-release.mjs.
 * @param {boolean} [options.apply] Whether GitHub mutations are enabled.
 * @param {string} [options.repo] GitHub owner/repository.
 * @param {object} [options.env] Environment used for the CI-only apply guard.
 * @param {(args: string[]) => Promise<string>} [options.gh] Injectable `gh` runner.
 * @param {(line: string) => void} [options.log]
 * @param {(line: string) => void} [options.error]
 * @returns {Promise<{exitCode: number, outcome: string}>}
 */
export async function fileWindReleaseReview(options = {}) {
  const {
    report,
    apply = false,
    repo: repoOption,
    env = process.env,
    gh = runGh,
    log = console.log,
    error = console.error,
  } = options;

  let validReport;
  try {
    validReport = validateReport(report);
  } catch (caught) {
    error(`malformed report: ${caught.message}`);
    return { exitCode: 1, outcome: "malformed-report" };
  }

  if (validReport.status !== "REVIEW_NEEDED") {
    log(`nothing to file (${validReport.status})`);
    return { exitCode: 0, outcome: "nothing-to-file" };
  }

  let repo;
  try {
    repo = validateRepo(repoOption ?? env.GITHUB_REPOSITORY ?? PROJECT);
  } catch (caught) {
    error(caught.message);
    return { exitCode: 1, outcome: "invalid-repo" };
  }
  const dryRun = env.DRY_RUN === "1" || !apply;
  if (dryRun) {
    printDryRunPlan(validReport, repo, log);
    return { exitCode: 0, outcome: "dry-run" };
  }
  if (env.GITHUB_ACTIONS !== "true" && env.WIND_RELEASE_FILER_ALLOW_LOCAL_APPLY !== "1") {
    error(
      "apply is CI-only; set GITHUB_ACTIONS=true or WIND_RELEASE_FILER_ALLOW_LOCAL_APPLY=1 for an authorized run",
    );
    return { exitCode: 1, outcome: "apply-blocked" };
  }

  try {
    const identity = issueMarker(validReport);
    let matches = await findMatchingIssues(gh, repo, validReport, identity);
    const openMatches = matches.filter((issue) => String(issue.state).toLowerCase() === "open");
    if (openMatches.length > 0) {
      await reconcileDuplicates(gh, repo, openMatches);
      const canonical = openMatches
        .slice()
        .sort((left, right) => Number(left.number) - Number(right.number))[0];
      const outcome = await refreshOpenIssue(gh, repo, canonical, validReport);
      if (outcome === "commented") log(`updated review issue #${canonical.number}`);
      return { exitCode: 0, outcome };
    }

    if (matches.length > 0) {
      const reviewed = matches
        .slice()
        .sort((left, right) => Number(left.number) - Number(right.number))[0];
      log(`already-reviewed-by-human #${reviewed.number}`);
      return { exitCode: 0, outcome: "already-reviewed-by-human" };
    }

    await ensureReviewLabel(gh, repo);
    const title = buildReviewTitle(validReport);
    const body = buildReviewBody(validReport);
    const createdStdout = await gh([
      "api",
      endpoint(repo, "issues"),
      "--method",
      "POST",
      "-f",
      `title=${title}`,
      "-f",
      `body=${body}`,
      "-f",
      `labels[]=${REVIEW_LABEL}`,
    ]);
    const created = parseObject(createdStdout);
    if (Number.isInteger(created?.number)) log(`filed review issue #${created.number}`);
    else log("filed review issue");

    matches = matchingIssues(await listReviewIssues(gh, repo), identity);
    await reconcileDuplicates(gh, repo, matches);
    return { exitCode: 0, outcome: "created" };
  } catch (caught) {
    error(errorText(caught));
    return { exitCode: 1, outcome: "gh-failure" };
  }
}

export function parseCliArgs(argv) {
  let reportPath;
  let apply = false;
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--report") {
      reportPath = argv[index + 1];
      if (!reportPath || reportPath.startsWith("--"))
        throw new Error("--report requires a file path");
      index += 1;
    } else if (arg === "--apply") {
      if (argv[index + 1] !== "yes") throw new Error("--apply only accepts yes");
      apply = true;
      index += 1;
    } else {
      throw new Error(`unknown argument ${arg}`);
    }
  }
  if (!reportPath)
    throw new Error(
      "usage: node scripts/file-wind-release-review.mjs --report <report.json> [--apply yes]",
    );
  return { reportPath, apply };
}

export async function runCli(argv = process.argv.slice(2), options = {}) {
  const log = options.log ?? console.log;
  const error = options.error ?? console.error;
  let parsed;
  try {
    parsed = parseCliArgs(argv);
  } catch (caught) {
    error(caught.message);
    return 1;
  }

  let report;
  try {
    report = JSON.parse(await readFile(parsed.reportPath, "utf8"));
  } catch (caught) {
    error(`malformed report: ${caught.message}`);
    return 1;
  }

  const result = await fileWindReleaseReview({ ...options, ...parsed, report, log, error });
  return result.exitCode;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await runCli();
}
