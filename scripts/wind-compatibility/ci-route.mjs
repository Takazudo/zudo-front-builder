#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";

// This is an allowlist of paths which cannot affect the compatibility harness.
// Unknown paths deliberately run the browser lane. Admission's tested-input
// identity is wider: it snapshots every tracked/unignored file except state.
export function irrelevant(path) {
  return (
    path === "README.md" ||
    path.startsWith(".github/ISSUE_TEMPLATE/") ||
    path.startsWith(".github/DISCUSSION_TEMPLATE/")
  );
}

export function route({ event, before, base, full, paths }) {
  if (event === "workflow_dispatch") {
    if (full !== "true") throw Error("Manual Wind admission requires wind_full=true");
    return { relevant: true, mode: "full" };
  }
  if (event === "push") return { relevant: true, mode: "full" };
  if (event !== "pull_request" || !/^[0-9a-f]{40}$/.test(base))
    throw Error("Unknown event or missing exact PR base SHA");
  if (!Array.isArray(paths) || paths.length === 0)
    throw Error("Changed-file list missing or empty");
  return { relevant: paths.some((path) => !irrelevant(path)), mode: "chromium" };
}

function main() {
  const outputArg = process.argv.indexOf("--github-output");
  if (outputArg < 0 || !process.argv[outputArg + 1]) throw Error("Missing GitHub output path");
  const event = process.env.EVENT_NAME;
  const base = process.env.PR_BASE_SHA;
  const paths =
    event === "pull_request"
      ? execFileSync("git", ["diff", "--name-only", "-z", `${base}...HEAD`])
          .toString()
          .split("\0")
          .filter(Boolean)
      : [];
  const result = route({
    event,
    before: process.env.BEFORE_SHA,
    base,
    full: process.env.WIND_FULL,
    paths,
  });
  appendFileSync(process.argv[outputArg + 1], `relevant=${result.relevant}\nmode=${result.mode}\n`);
  console.log(JSON.stringify({ ...result, changedFiles: paths }));
}
if (process.argv[1] && import.meta.filename === process.argv[1]) main();
