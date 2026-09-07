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
 * legitimate drift and are meant to trip `--strict`.
 *
 * Wire shape (#2933): `env=v<N>:sha256:<16 hex>`. `<N>` is
 * `ENV_IDENTITY_VERSION`, the version of the *contract* that produced the
 * digest -- which keys `STEERING_ENV_KEYS` lists, in which order, serialized
 * how. Two digests are comparable only under the same version, so changing
 * `STEERING_ENV_KEYS` (or its order, or the serialization) is a version bump
 * here, and nothing else: the summarizer splits its population into
 * per-version cohorts and never pools digests across them, so no date epoch
 * has to be picked and no already-emitted record has to be re-read. A record
 * carrying an unversioned `env=sha256:…` predates the prefix and reads as
 * **v1** -- the same contract the prefix now makes explicit, so a population
 * straddling #2933 shows no drift at all. (v1's unversioned members do
 * include pre-#2913 records emitted under the old full-environment rule;
 * nothing on the wire distinguishes those, they simply look like drift
 * inside v1 until the harvest window rolls past them.)
 */

// The contract version stamped into every emitted `env=` token. Bump it
// whenever STEERING_ENV_KEYS, their order, or the serialization below
// changes -- that is the whole mechanism by which the summarizer keeps the
// old and new populations apart.
export const ENV_IDENTITY_VERSION = 1;

// Environment entries that actually steer this spawn: the two run-parallel reads
// to pick a package manager, plus the ones that change how node/pnpm start up.
// `npm_execpath` is always <unset> here by construction (spawnSupervisor
// deletes it); the ambient value vitest itself was launched with is reported
// separately on its own line. Order matters: it is the serialization order of
// the digest, so reordering it is a contract change (bump ENV_IDENTITY_VERSION).
export const STEERING_ENV_KEYS = [
  "npm_execpath",
  "npm_config_user_agent",
  "NODE_OPTIONS",
  "NODE_ENV",
  "CI",
  "PNPM_HOME",
  "TMPDIR",
];

// Which vitest worker we are sharing the machine with. Vitest reassigns these
// per run purely from worker scheduling -- a single-file run and a full-suite
// run of the same commit differ in nothing else. Folding them into the env
// digest would make every baseline-vs-load comparison report input drift that
// is not there, so they are reported by name and excluded from the comparator.
const VOLATILE_ENV_KEY_LIST = ["VITEST_POOL_ID", "VITEST_WORKER_ID", "TINYPOOL_WORKER_ID"];
export const VOLATILE_ENV_KEYS = new Set(VOLATILE_ENV_KEY_LIST);

// Everything the failure-evidence block prints by name: the steering keys,
// then the volatile ones.
export const REPORTED_ENV_KEYS = [...STEERING_ENV_KEYS, ...VOLATILE_ENV_KEY_LIST];

export function digestOf(text) {
  return `sha256:${createHash("sha256").update(text).digest("hex").slice(0, 16)}`;
}

/** The bare digest over `keys` (default: the live STEERING_ENV_KEYS). */
export function steeringEnvDigest(env, keys = STEERING_ENV_KEYS) {
  const serialized = keys
    .map((key) => `${key}=${env[key] === undefined ? "<unset>" : env[key]}`)
    .join("\n");
  return digestOf(serialized);
}

/** `v<version>:<digest>` -- the canonical form of every `env=` token. */
export function envIdentityToken(version, digest) {
  return `v${version}:${digest}`;
}

/**
 * The `env=` token the emitter writes: the live contract version over the
 * live steering keys. `keys` and `version` are injectable only so a test can
 * stage the *next* contract beside the current one; production callers pass
 * nothing.
 */
export function steeringEnvIdentity(
  env,
  { keys = STEERING_ENV_KEYS, version = ENV_IDENTITY_VERSION } = {},
) {
  return envIdentityToken(version, steeringEnvDigest(env, keys));
}

const VERSIONED_TOKEN = /^v([1-9]\d*):(.+)$/;

/**
 * Reads an `env=` token back. An unversioned token is v1 by declaration (see
 * the header), so `sha256:X` and `v1:sha256:X` yield the same `{ version,
 * digest }` and the same canonical `token`. Never throws: `env` is a
 * string-valued identity field and an unexpected value must surface as drift
 * in the report, not take the whole analysis down.
 */
export function parseEnvIdentity(raw) {
  const match = VERSIONED_TOKEN.exec(raw);
  const version = match ? Number(match[1]) : 1;
  const digest = match ? match[2] : raw;
  return { version, digest, token: envIdentityToken(version, digest) };
}
