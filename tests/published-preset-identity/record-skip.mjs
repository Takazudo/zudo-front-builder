#!/usr/bin/env node
// Records why health skipped the published-preset fixture (#4177). See README.md.
//
// health.yml runs this only when the fixture step's outcome is `skipped`: a
// prerequisite (branch binary build, Chromium install) did not succeed, or the
// job was cancelled. The record is evidence of a skip, never a pass, so it
// refuses to describe a state in which the fixture should have run.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const PREREQUISITES = ["build_binary", "chromium"];

export function skipRecord({ outcomes, jobStatus }) {
  const unmet = PREREQUISITES.filter((name) => outcomes[name] !== "success");
  const cancelled = jobStatus === "cancelled";
  if (!cancelled && unmet.length === 0) {
    throw new Error(
      "published-preset fixture was skipped although every prerequisite succeeded and the job was not cancelled",
    );
  }
  return {
    issue: 3331,
    status: "skipped",
    reason: cancelled ? "cancelled" : "prerequisite-not-met",
    prerequisites: Object.fromEntries(PREREQUISITES.map((name) => [name, outcomes[name] ?? null])),
    unmet,
    jobStatus: jobStatus ?? null,
  };
}

function main() {
  const record = skipRecord({
    outcomes: {
      build_binary: process.env.PREREQ_BUILD_BINARY,
      chromium: process.env.PREREQ_CHROMIUM,
    },
    jobStatus: process.env.JOB_STATUS,
  });
  const evidenceRoot =
    process.env.ZFB_PRESET_EVIDENCE_DIR ||
    join(process.env.RUNNER_TEMP || tmpdir(), "published-preset-identity");
  mkdirSync(evidenceRoot, { recursive: true });
  writeFileSync(join(evidenceRoot, "evidence.json"), JSON.stringify(record, null, 2) + "\n");
  const detail = Object.entries(record.prerequisites)
    .map(([name, outcome]) => `${name}=${outcome}`)
    .join(" ");
  const line = `published-preset fixture SKIPPED (not passed): ${record.reason}; ${detail}`;
  console.log(`::notice title=Published-preset fixture skipped::${line}`);
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `- ${line}\n`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
