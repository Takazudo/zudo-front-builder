import { createHash } from "node:crypto";

/**
 * Identity of the environment a supervised `run-parallel.mjs` spawn saw --
 * the `env=` field of a `[supervisor-timeline]` record (emitted by
 * `scripts/__tests__/docs-dev-supervisor.test.mjs`) and the input `env`
 * comparator `scripts/supervisor-timeline-summary.mjs` uses for its drift
 * check.
 *
 * Volatile keys (vitest worker-scheduling assignments) and non-steering keys
 * (everything else ambient in the process environment -- CI run ids, shell
 * history state, temp-file paths the harness itself created) are excluded
 * because #2913 found they made the field useless: on GitHub Actions the
 * full-environment digest embedded `GITHUB_RUN_ID`, `GITHUB_SHA`, and
 * `GITHUB_ENV` temp paths, so it never matched across two runs of identical
 * config even though nothing that steers the spawn had changed.
 *
 * The resulting contract is **steering-only, not non-drifting**: the digest
 * changes exactly when a steering input changes -- e.g. a node patch bump
 * (`npm_config_user_agent` embeds `process.versions.node`), a runner-image
 * change to `PNPM_HOME` / `TMPDIR`, or a new `NODE_OPTIONS`. Those are
 * legitimate drift and are meant to trip `--strict`. Changing
 * `STEERING_ENV_KEYS` itself changes every future digest -- populations
 * spanning such a change must be split or harvested with `--allow-drift env`.
 */

// Environment entries that actually steer this spawn: the two run-parallel reads
// to pick a package manager, plus the ones that change how node/pnpm start up or
// which vitest worker we are sharing the machine with. `npm_execpath` is always
// <unset> here by construction (spawnSupervisor deletes it); the ambient value
// vitest itself was launched with is reported separately on its own line.
export const REPORTED_ENV_KEYS = [
  "npm_execpath",
  "npm_config_user_agent",
  "NODE_OPTIONS",
  "NODE_ENV",
  "CI",
  "PNPM_HOME",
  "TMPDIR",
  "VITEST_POOL_ID",
  "VITEST_WORKER_ID",
  "TINYPOOL_WORKER_ID",
];

// Vitest reassigns these per run purely from worker scheduling -- a single-file
// run and a full-suite run of the same commit differ in nothing else. Folding
// them into the env digest would make every baseline-vs-load comparison report
// input drift that is not there, so they are reported by name and excluded from
// the comparator.
export const VOLATILE_ENV_KEYS = new Set([
  "TINYPOOL_WORKER_ID",
  "VITEST_POOL_ID",
  "VITEST_WORKER_ID",
]);

export const STEERING_ENV_KEYS = REPORTED_ENV_KEYS.filter((key) => !VOLATILE_ENV_KEYS.has(key));

export function digestOf(text) {
  return `sha256:${createHash("sha256").update(text).digest("hex").slice(0, 16)}`;
}

export function steeringEnvDigest(env) {
  const serialized = STEERING_ENV_KEYS.map(
    (key) => `${key}=${env[key] === undefined ? "<unset>" : env[key]}`,
  ).join("\n");
  return digestOf(serialized);
}
