// Shared `gh` runner for the scripts/release/* helpers. Every helper takes an
// injectable runner so the vitest suites never reach GitHub; the CLI default
// shells out to whatever `gh` is first on PATH (the subprocess suites put a
// stub there).
import { execFileSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

export const DEFAULT_REPO = "Takazudo/zudo-front-builder";

export class GhError extends Error {
  constructor(args, error) {
    const stderr = String(error?.stderr ?? "").trim();
    super(`gh ${args.join(" ")} failed: ${stderr || error?.message || "unknown error"}`);
    this.stderr = stderr;
    this.status = error?.status ?? null;
    this.notFound = /\bHTTP 404\b|Not Found|release not found/i.test(stderr);
  }
}

export function execGh(args, { input } = {}) {
  try {
    return execFileSync("gh", args, {
      encoding: "utf8",
      input,
      stdio: [input === undefined ? "ignore" : "pipe", "pipe", "pipe"],
      maxBuffer: 256 * 1024 * 1024,
    });
  } catch (error) {
    throw new GhError(args, error);
  }
}

/** Parse newline-delimited JSON, as `gh api --paginate --jq '.x[]'` prints it. */
export function parseJsonLines(text) {
  return String(text)
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

export function repoFromEnv(env = process.env) {
  return env.ZFB_RELEASE_REPO || env.GITHUB_REPOSITORY || DEFAULT_REPO;
}

function resolved(path) {
  try {
    return realpathSync(path);
  } catch {
    return null;
  }
}

/**
 * True only when this module is the script node was asked to run. Under
 * `node -e`/`-p`/`--eval` the first extra argument becomes argv[1], which once
 * made a test run a live CLI by naming its file there; refuse that shape.
 */
export function isMain(importMetaUrl, argv = process.argv, execArgv = process.execArgv) {
  if (execArgv.some((arg) => /^(-e|-p|--eval|--print)(=|$)/.test(arg))) return false;
  const invoked = argv[1] && resolved(argv[1]);
  return Boolean(invoked) && invoked === resolved(fileURLToPath(importMetaUrl));
}
