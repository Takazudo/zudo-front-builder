#!/usr/bin/env node
/**
 * Dispatch-only Tailwind release detection. A channel change appears through
 * offTrackNewest / prerelease-only; it never becomes an on-track candidate.
 * Local runs are read-only and always emit a deterministic report.
 */
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import {
  compareVersion,
  exactVersion,
  fromRoot,
  paths,
  requireChannel,
} from "./wind-compatibility/reference.mjs";

const PACKAGE = "tailwindcss";
const REGISTRY_URL = `https://registry.npmjs.org/${PACKAGE}`;
const TAG_URL = "https://api.github.com/repos/tailwindlabs/tailwindcss/git/ref/tags/";
const EXIT_CODES = {
  "no-change": 0,
  "already-reviewed": 0,
  "prerelease-only": 0,
  REVIEW_NEEDED: 10,
  "operational-failure": 1,
};

class RequestError extends Error {
  constructor(message, httpStatus = null) {
    super(message);
    this.httpStatus = httpStatus;
  }
}

export async function requestJson(
  url,
  {
    headers = {},
    fetchImpl = fetch,
    sleepImpl = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    timeoutMs = 10_000,
    retries = 2,
  } = {},
) {
  for (let attempt = 0; attempt <= retries; attempt++) {
    const controller = new AbortController();
    let timer;
    try {
      return await Promise.race([
        (async () => {
          const response = await fetchImpl(url, { headers, signal: controller.signal });
          if (!response.ok)
            throw new RequestError(`HTTP ${response.status}: ${url}`, response.status);
          let body;
          try {
            body = await response.text();
          } catch (error) {
            throw new RequestError(`Response read failed: ${url}: ${error.message}`);
          }
          try {
            return JSON.parse(body);
          } catch (error) {
            throw new RequestError(`Invalid JSON: ${url}: ${error.message}`, response.status);
          }
        })(),
        new Promise((_, reject) => {
          timer = setTimeout(() => {
            controller.abort();
            reject(new RequestError(`Request timed out: ${url}`));
          }, timeoutMs);
        }),
      ]);
    } catch (error) {
      const status = error.httpStatus ?? null;
      if (attempt === retries || (status !== null && status !== 429 && status < 500)) {
        throw error instanceof RequestError ? error : new RequestError(error.message);
      }
      await sleepImpl(100 * 2 ** attempt);
    } finally {
      clearTimeout(timer);
    }
  }
}

async function loadRecords() {
  const [profile, acceptedFile, reviewedFile] = await Promise.all(
    [paths.profile, paths.accepted, paths.reviewed].map(async (path) =>
      JSON.parse(await readFile(fromRoot(path), "utf8")),
    ),
  );
  return { profile, acceptedFile, reviewedFile };
}

function validateRecords({ profile, acceptedFile, reviewedFile }) {
  const accepted = acceptedFile?.acceptedReference;
  const reviewed = reviewedFile?.reviewedThrough;
  if (
    profile?.schemaVersion !== 1 ||
    typeof profile.profileId !== "string" ||
    !profile.profileId ||
    acceptedFile?.schemaVersion !== 1 ||
    reviewedFile?.schemaVersion !== 1 ||
    accepted?.package !== PACKAGE ||
    reviewed?.package !== PACKAGE ||
    accepted?.profileId !== profile.profileId ||
    !/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(accepted?.integrity ?? "") ||
    !/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(reviewed?.integrity ?? "") ||
    reviewed?.disposition !== "review-only"
  )
    throw Error("Invalid Tailwind reference records");
  exactVersion(accepted.version);
  exactVersion(reviewed.version);
  requireChannel(accepted.version, accepted.channel);
  requireChannel(reviewed.version, reviewed.channel);
  if (
    accepted.channel !== reviewed.channel ||
    compareVersion(reviewed.version, accepted.version) < 0
  )
    throw Error("Reference channels or version order disagree");
  return {
    profileId: profile.profileId,
    accepted: {
      version: accepted.version,
      channel: accepted.channel,
      integrity: accepted.integrity,
    },
    reviewedThrough: {
      version: reviewed.version,
      channel: reviewed.channel,
      disposition: reviewed.disposition,
    },
  };
}

function identity(report) {
  const value = {
    package: report.package,
    profileId: report.profileId,
    track: report.track,
    accepted: { version: report.accepted?.version ?? null },
    reviewedThrough: { version: report.reviewedThrough?.version ?? null },
    candidate: {
      version: report.candidate?.version ?? null,
      integrity: report.candidate?.integrity ?? null,
    },
  };
  // Canonical JSON: keys sorted recursively, with no whitespace.
  const canonical = (x) =>
    x && typeof x === "object" && !Array.isArray(x)
      ? Object.fromEntries(
          Object.keys(x)
            .sort()
            .map((key) => [key, canonical(x[key])]),
        )
      : x;
  return `sha256:${createHash("sha256")
    .update(JSON.stringify(canonical(value)))
    .digest("hex")}`;
}

function finish(report, status, failure = null) {
  report.status = status;
  report.exitCode = EXIT_CODES[status];
  report.failure = failure;
  report.identity = identity(report);
  return report;
}

function failure(report, stage, error) {
  report.candidate = null;
  return finish(report, "operational-failure", {
    stage,
    message: error.message,
    httpStatus: error.httpStatus ?? null,
  });
}

function sortedVersions(versions) {
  return versions.sort((a, b) => compareVersion(a, b));
}

function sourceFromTag(body, version) {
  const tag = `v${version}`;
  if (
    body?.ref !== `refs/tags/${tag}` ||
    !["commit", "tag"].includes(body.object?.type) ||
    !/^[0-9a-f]{40}$/.test(body.object?.sha ?? "")
  )
    throw Error(`Malformed Git tag reference for ${tag}`);
  return {
    tag,
    observedTagObject: { type: body.object.type, sha: body.object.sha },
    status:
      body.object.type === "commit"
        ? "tag-observed-artifact-link-unverified"
        : "tag-object-unresolved",
  };
}

export async function detectTailwindRelease({
  records,
  fetchImpl = fetch,
  sleepImpl,
  timeoutMs,
  retries,
  githubToken = process.env.GITHUB_TOKEN,
} = {}) {
  const report = {
    schemaVersion: 1,
    kind: "wind-release-detection",
    status: "operational-failure",
    exitCode: 1,
    package: PACKAGE,
    profileId: null,
    track: null,
    accepted: null,
    reviewedThrough: null,
    latestDistTag: null,
    candidate: null,
    offTrackNewest: null,
    interveningVersions: [],
    interveningTruncated: false,
    notes: [],
    failure: null,
    identity: null,
  };
  try {
    const state = validateRecords(records ?? (await loadRecords()));
    Object.assign(report, state, { track: state.accepted.channel });
  } catch (error) {
    return failure(report, "records", error);
  }

  let packument;
  try {
    packument = await requestJson(REGISTRY_URL, {
      headers: { Accept: "application/vnd.npm.install-v1+json" },
      fetchImpl,
      sleepImpl,
      timeoutMs,
      retries,
    });
    if (
      !packument?.versions ||
      typeof packument.versions !== "object" ||
      Array.isArray(packument.versions)
    )
      throw Error("Packument has no versions object");
    report.latestDistTag = packument["dist-tags"]?.latest ?? null;
    const parseable = [];
    let skipped = 0;
    for (const [version, meta] of Object.entries(packument.versions)) {
      try {
        exactVersion(version);
      } catch {
        skipped++;
        continue;
      }
      if (
        meta?.version !== version ||
        !/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(meta.dist?.integrity ?? "") ||
        !/^https:\/\//.test(meta.dist?.tarball ?? "")
      )
        throw Error(`Missing or invalid artifact metadata for ${version}`);
      parseable.push(version);
    }
    if (!parseable.length) throw Error("Packument has no exactVersion-parseable versions");
    if (skipped) report.notes.push(`Skipped ${skipped} unparseable version strings.`);
    const onTrack = sortedVersions(
      parseable.filter(
        (v) =>
          (exactVersion(v).prerelease ? "prerelease" : "stable") === report.track &&
          compareVersion(v, report.accepted.version) > 0,
      ),
    );
    const offTrack = sortedVersions(
      parseable.filter(
        (v) =>
          (exactVersion(v).prerelease ? "prerelease" : "stable") !== report.track &&
          compareVersion(v, report.accepted.version) > 0,
      ),
    );
    if (offTrack.length) {
      const version = offTrack.at(-1);
      report.offTrackNewest = {
        version,
        channel: exactVersion(version).prerelease ? "prerelease" : "stable",
      };
      report.notes.push(`Newest off-track release: ${version} (${report.offTrackNewest.channel}).`);
    }
    if (report.latestDistTag) {
      try {
        if (exactVersion(report.latestDistTag).prerelease)
          report.notes.push(`Latest dist-tag points at a prerelease: ${report.latestDistTag}.`);
        if (onTrack.length && compareVersion(report.latestDistTag, onTrack.at(-1)) < 0)
          report.notes.push(
            `Latest dist-tag ${report.latestDistTag} is below newest on-track ${onTrack.at(-1)}.`,
          );
      } catch {
        report.notes.push(`Latest dist-tag is unparseable: ${report.latestDistTag}.`);
      }
    }
    if (!onTrack.length) return finish(report, offTrack.length ? "prerelease-only" : "no-change");
    const newest = onTrack.at(-1);
    report.interveningVersions = onTrack.slice(0, 30);
    report.interveningTruncated = onTrack.length > 30;
    const meta = packument.versions[newest];
    const accepted = exactVersion(report.accepted.version);
    const selected = exactVersion(newest);
    report.candidate = {
      version: newest,
      channel: report.track,
      integrity: meta.dist.integrity,
      tarball: meta.dist.tarball,
      sha1: meta.dist.shasum ?? null,
      transition: {
        major: selected.major > accepted.major,
        minor: selected.major === accepted.major && selected.minor > accepted.minor,
        patch:
          selected.major === accepted.major &&
          selected.minor === accepted.minor &&
          selected.patch > accepted.patch,
      },
      source: null,
    };
    if (compareVersion(newest, report.reviewedThrough.version) <= 0)
      return finish(report, "already-reviewed");
  } catch (error) {
    return failure(report, "registry", error);
  }

  try {
    const version = report.candidate.version;
    const headers = { Accept: "application/vnd.github+json" };
    if (githubToken) headers.Authorization = `Bearer ${githubToken}`;
    const body = await requestJson(`${TAG_URL}v${encodeURIComponent(version)}`, {
      headers,
      fetchImpl,
      sleepImpl,
      timeoutMs,
      retries,
    });
    report.candidate.source = sourceFromTag(body, version);
  } catch (error) {
    if (error.httpStatus === 404) {
      report.candidate.source = {
        tag: `v${report.candidate.version}`,
        observedTagObject: null,
        status: "tag-missing",
      };
    } else return failure(report, "tag", error);
  }
  return finish(report, "REVIEW_NEEDED");
}

export function renderReport(report) {
  if (report?.kind !== "wind-release-detection" || report.exitCode !== EXIT_CODES[report.status])
    throw Error("Invalid detection report");
  const lines = [
    `Tailwind release detection: ${report.status}`,
    `Track: ${report.package} / ${report.profileId} / ${report.track}`,
    `Accepted: ${report.accepted?.version ?? "unknown"} → Candidate: ${report.candidate?.version ?? "none"}`,
    `Intervening versions: ${report.interveningVersions.length}${report.interveningTruncated ? "+" : ""}`,
  ];
  if (report.notes.length) lines.push("Notes:", ...report.notes.map((note) => `- ${note}`));
  if (report.failure) lines.push(`Failure (${report.failure.stage}): ${report.failure.message}`);
  return `${lines.join("\n")}\n`;
}

export async function runCli(
  args = process.argv.slice(2),
  stdout = process.stdout,
  stderr = process.stderr,
) {
  if (args.length === 2 && args[0] === "detect" && args[1] === "--json") {
    const report = await detectTailwindRelease();
    stdout.write(`${JSON.stringify(report)}\n`);
    return report.exitCode;
  }
  if (args.length === 2 && args[0] === "--render") {
    try {
      stdout.write(renderReport(JSON.parse(await readFile(args[1], "utf8"))));
      return 0;
    } catch (error) {
      stderr.write(`${error.message}\n`);
      return 1;
    }
  }
  stderr.write("Usage: detect-tailwind-release.mjs detect --json | --render <report.json>\n");
  return 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  process.exitCode = await runCli();
