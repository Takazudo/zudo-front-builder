#!/usr/bin/env node
// #4200: record only a genuinely skipped #3931 probe. Executed diagnostics
// own results.json; exclusive creation keeps this recorder from replacing them.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
// Compare canonical paths: a symlinked invocation path (macOS /var -> /private/var) must still run main() (#4465).
import { realpathSync as realpathSyncForMain } from "node:fs";

const PREREQUISITES = ["build_binary", "chromium"];

export function skipRecord({ probeOutcome, outcomes, jobStatus, identity = {} }) {
  if (probeOutcome !== "skipped") {
    throw new Error(`linked-package probe outcome is ${probeOutcome ?? "unknown"}, not skipped`);
  }
  const unmet = PREREQUISITES.filter((name) => outcomes[name] !== "success");
  const cancelled = jobStatus === "cancelled";
  if (!cancelled && unmet.length === 0) {
    throw new Error(
      "linked-package probe was skipped although every prerequisite succeeded and the job was not cancelled",
    );
  }
  return {
    issue: 3931,
    gateIssue: 4200,
    status: "skipped",
    reason: cancelled ? "cancelled" : "prerequisite-not-met",
    prerequisites: Object.fromEntries(PREREQUISITES.map((name) => [name, outcomes[name] || null])),
    unmet,
    jobStatus: jobStatus || null,
    checkoutSha: identity.sha || null,
    run: {
      repository: identity.repository || null,
      id: identity.runId || null,
      attempt: identity.runAttempt || null,
      job: identity.job || null,
      ref: identity.ref || null,
      url:
        identity.serverUrl && identity.repository && identity.runId
          ? `${identity.serverUrl}/${identity.repository}/actions/runs/${identity.runId}`
          : null,
    },
  };
}

function main() {
  const record = skipRecord({
    probeOutcome: process.env.PROBE_OUTCOME,
    outcomes: {
      build_binary: process.env.PREREQ_BUILD_BINARY,
      chromium: process.env.PREREQ_CHROMIUM,
    },
    jobStatus: process.env.JOB_STATUS,
    identity: {
      sha: process.env.GITHUB_SHA,
      repository: process.env.GITHUB_REPOSITORY,
      runId: process.env.GITHUB_RUN_ID,
      runAttempt: process.env.GITHUB_RUN_ATTEMPT,
      job: process.env.GITHUB_JOB,
      ref: process.env.GITHUB_REF,
      serverUrl: process.env.GITHUB_SERVER_URL,
    },
  });
  const evidenceRoot =
    process.env.ZFB_PROBE_EVIDENCE_DIR ||
    join(process.env.RUNNER_TEMP || tmpdir(), "linked-package-token-probe");
  mkdirSync(evidenceRoot, { recursive: true });
  writeFileSync(join(evidenceRoot, "results.json"), JSON.stringify(record, null, 2) + "\n", {
    flag: "wx",
  });
  const detail = Object.entries(record.prerequisites)
    .map(([name, outcome]) => `${name}=${outcome ?? "unknown"}`)
    .join(" ");
  const line = `linked-package token probe SKIPPED (not passed): ${record.reason}; ${detail}`;
  console.log(`::notice title=Linked-package token probe skipped::${line}`);
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `- ${line}\n`);
  }
}

if (
  process.argv[1] &&
  realpathSyncForMain(process.argv[1]) === realpathSyncForMain(fileURLToPath(import.meta.url))
) {
  main();
}
