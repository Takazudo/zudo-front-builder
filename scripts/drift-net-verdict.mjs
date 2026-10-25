#!/usr/bin/env node
// Drift net tracking-issue verdict (#4533). Pure decision over the five job
// results so the truth table is unit-testable instead of living in YAML.
//
// Usage: node scripts/drift-net-verdict.mjs --legs <all|preset-carrier|""> \
//          --result <job>=<result> ... [--github-output <file>]
// Prints `action=<file|close|none>` and `failed=<comma list>`.

import { appendFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export const DRIFT_NET_JOBS = [
  "smoke-latest-ubuntu",
  "smoke-latest-macos",
  "provenance-latest",
  "preset-installed-carrier-ubuntu",
  "preset-installed-carrier-windows",
];

// A partial dispatch never touches the tracking issue; a skipped job is NOT RUN
// and never counts toward closing; only five successes close.
export function driftNetVerdict({ legs, results }) {
  if (legs === "preset-carrier") return { action: "none", failed: [] };
  const failed = DRIFT_NET_JOBS.filter(
    (job) => results[job] === "failure" || results[job] === "cancelled",
  );
  if (failed.length > 0) return { action: "file", failed };
  if (DRIFT_NET_JOBS.every((job) => results[job] === "success")) {
    return { action: "close", failed: [] };
  }
  return { action: "none", failed: [] };
}

function main(argv) {
  let legs = "";
  let output = "";
  const results = {};
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--legs") legs = argv[++i] ?? "";
    else if (argv[i] === "--github-output") output = argv[++i] ?? "";
    else if (argv[i] === "--result") {
      const [job, ...rest] = (argv[++i] ?? "").split("=");
      results[job] = rest.join("=");
    } else throw new Error(`unknown argument: ${argv[i]}`);
  }
  const { action, failed } = driftNetVerdict({ legs, results });
  const lines = `action=${action}\nfailed=${failed.join(", ")}\n`;
  process.stdout.write(lines);
  if (output) appendFileSync(output, lines);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main(process.argv.slice(2));
