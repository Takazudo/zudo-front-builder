#!/usr/bin/env node
/**
 * npm-oidc-preflight.mjs — fail-closed guard run before every npm registry
 * write in the release path (publish, `dist-tag add`, `dist-tag rm`).
 *
 * The release path authenticates ONLY through GitHub OIDC trusted publishing.
 * npm (>= 12.2.0 for dist-tag) and pnpm exchange the job's GitHub ID token for
 * a short-lived npm token. Both clients fall back SILENTLY to whatever stored
 * credential they can find when that exchange fails, so a leftover token would
 * turn an OIDC misconfiguration into an unnoticed token-authenticated write.
 * This preflight therefore refuses to proceed when:
 *
 *   - the GitHub OIDC request variables are missing (the job lacks
 *     `permissions: id-token: write`), or
 *   - ANY stored npm credential is visible: NODE_AUTH_TOKEN / NPM_TOKEN,
 *     auth-shaped `npm_config_*` / `pnpm_config_*` variables, auth keys in any
 *     config layer `npm config list` reports (user, global, project, env), or
 *     auth keys in an `.npmrc` above the working directory or in pnpm's own
 *     config files (`rc`, `auth.ini`, `config.yaml`), or
 *   - the installed npm is older than 12.2.0 (no OIDC for dist-tag writes).
 *
 * Only key NAMES and source locations are ever reported — never values.
 *
 * Usage (CLI): node scripts/npm-oidc-preflight.mjs
 *   exit 0 = clear, exit 1 = refused (with ::error:: annotations).
 */

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

/** First npm release whose OIDC exchange also covers `npm dist-tag add/rm`. */
export const MIN_NPM_VERSION = "12.2.0";

const OIDC_ENV = ["ACTIONS_ID_TOKEN_REQUEST_URL", "ACTIONS_ID_TOKEN_REQUEST_TOKEN"];
const TOKEN_ENV = ["NODE_AUTH_TOKEN", "NPM_TOKEN"];

// A config key that carries a credential. Covers bare (`_auth`), nerf-darted
// (`//registry.npmjs.org/:_authToken`) and pnpm's `tokenHelper` forms.
const AUTH_KEY_RE = /(^|:)(_authtoken|_auth|_password|username|tokenhelper|certfile|keyfile)$/i;
const CONFIG_ENV_PREFIX_RE = /^(npm|pnpm)_config_/i;

export function isAuthKey(key) {
  return AUTH_KEY_RE.test(String(key ?? "").trim());
}

/** Environment findings: missing OIDC variables and credential-shaped names. */
export function inspectEnv(env) {
  const problems = [];
  for (const name of OIDC_ENV) {
    if (!env[name]) {
      problems.push(
        `${name} is not set — the job has no GitHub OIDC token (needs permissions: id-token: write).`,
      );
    }
  }
  for (const name of Object.keys(env)) {
    if (TOKEN_ENV.includes(name.toUpperCase())) {
      problems.push(`environment variable ${name} is defined (stored npm credential).`);
      continue;
    }
    if (CONFIG_ENV_PREFIX_RE.test(name) && isAuthKey(name.replace(CONFIG_ENV_PREFIX_RE, ""))) {
      problems.push(`environment variable ${name} is defined (npm/pnpm auth config).`);
    }
  }
  return problems;
}

/** Auth keys from ini-style text (`.npmrc`, pnpm `rc`/`auth.ini`) or `npm config list`. */
export function authKeysInIni(text) {
  const keys = [];
  for (const raw of String(text ?? "").split(/\r?\n/)) {
    const line = raw.trim();
    if (line === "" || line.startsWith(";") || line.startsWith("#")) continue;
    // ini (`key = value`) splits on the first "=", because nerf-darted keys
    // contain ":" themselves; pnpm's config.yaml (`key: value`) has no "=".
    const eq = line.indexOf("=");
    const rawKey = eq >= 0 ? line.slice(0, eq) : line.replace(/:\s.*$|:$/, "");
    const key = rawKey.trim().replace(/^["']|["']$/g, "");
    if (key !== "" && isAuthKey(key)) keys.push(key);
  }
  return keys;
}

/** Every config file npm or pnpm may read credentials from, for this cwd/env. */
export function candidateConfigFiles({ env, cwd, home }) {
  const files = new Set();
  let dir = resolve(cwd);
  for (;;) {
    files.add(join(dir, ".npmrc"));
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  files.add(join(home, ".npmrc"));
  for (const name of ["NPM_CONFIG_USERCONFIG", "npm_config_userconfig"]) {
    if (env[name]) files.add(resolve(env[name]));
  }
  const pnpmDirs = [join(home, ".config", "pnpm"), join(home, "Library", "Preferences", "pnpm")];
  if (env.XDG_CONFIG_HOME) pnpmDirs.unshift(join(env.XDG_CONFIG_HOME, "pnpm"));
  for (const d of pnpmDirs) {
    for (const f of ["rc", "auth.ini", "config.yaml"]) files.add(join(d, f));
  }
  return [...files];
}

/** True when `version` (x.y.z, prerelease ignored) is >= `min`. */
export function versionAtLeast(version, min) {
  const parse = (v) =>
    /^(\d+)\.(\d+)\.(\d+)/
      .exec(String(v ?? "").trim())
      ?.slice(1)
      .map(Number);
  const a = parse(version);
  const b = parse(min);
  if (!a || !b) return false;
  for (let i = 0; i < 3; i += 1) {
    if (a[i] !== b[i]) return a[i] > b[i];
  }
  return true;
}

function defaultNpm(args) {
  return execFileSync("npm", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

/**
 * Run every check. Returns the list of problems (empty = clear). Each
 * dependency is injectable so the tests never touch the real machine config.
 */
export function npmOidcPreflight({
  env = process.env,
  cwd = process.cwd(),
  home = env.HOME || homedir(),
  npm = defaultNpm,
  fileExists = existsSync,
  readFile = (p) => readFileSync(p, "utf8"),
} = {}) {
  const problems = inspectEnv(env);

  for (const file of candidateConfigFiles({ env, cwd, home })) {
    let text;
    try {
      if (!fileExists(file)) continue;
      text = readFile(file);
    } catch (error) {
      problems.push(`cannot read ${file} to rule out stored credentials (${error.message}).`);
      continue;
    }
    for (const key of authKeysInIni(text)) {
      problems.push(`${file} defines ${key} (stored npm credential).`);
    }
  }

  let listing;
  try {
    listing = npm(["config", "list"]);
  } catch (error) {
    problems.push(
      `\`npm config list\` failed, so stored credentials cannot be ruled out (${error.message}).`,
    );
  }
  if (listing !== undefined) {
    for (const key of authKeysInIni(listing)) {
      problems.push(`npm's effective config contains ${key} (stored npm credential).`);
    }
  }

  let version;
  try {
    version = String(npm(["--version"])).trim();
  } catch (error) {
    problems.push(`\`npm --version\` failed (${error.message}).`);
  }
  if (version !== undefined && !versionAtLeast(version, MIN_NPM_VERSION)) {
    problems.push(
      `npm ${version || "(unknown)"} is older than ${MIN_NPM_VERSION}, which lacks OIDC for dist-tag writes.`,
    );
  }

  return problems;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const problems = npmOidcPreflight();
  if (problems.length > 0) {
    for (const problem of problems) console.log(`::error title=npm OIDC preflight::${problem}`);
    console.log(
      "::error title=npm OIDC preflight::Refusing to write to the npm registry. The release path authenticates only through GitHub OIDC trusted publishing; remove every stored npm credential and run inside the release workflow.",
    );
    process.exit(1);
  }
  console.log("npm OIDC preflight: clear (GitHub OIDC available, no stored npm credential found).");
}
