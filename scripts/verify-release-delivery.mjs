#!/usr/bin/env node
/**
 * verify-release-delivery.mjs — read-only audit of one published zfb release.
 *
 * Usage:
 *   node scripts/verify-release-delivery.mjs \
 *     --version 5.2.2 \
 *     --release-run-id 42176242188 \
 *     --installer-run-id 42181081547 \
 *     --json release-delivery-5.2.2.json
 *
 * Optional:
 *   --repo OWNER/NAME          GitHub repository (default Takazudo/zudo-front-builder)
 *   --expected-source SHA      Also require the tag to resolve to this commit
 *   --expect-github-latest     Require GitHub "Latest" to be this release
 *   --expect-npm-latest        Require the `latest` dist-tag of all eleven packages to be this version
 *   --expect-homebrew          Require each stable Homebrew OS/CPU row to download this version's archive for that target with its verified digest
 *   --timeout-ms N             Per-request timeout (default 60000)
 *   --retries N                Retries for network/5xx/rate-limit answers (default 3)
 *   --concurrency N            Parallel downloads (default 2, max 4)
 *
 * What it proves, per section of the report:
 *   identity     the tag's commit (annotated tags peeled), the GitHub Release
 *                metadata and packages/zfb/package.json at that commit
 *   workflow     the selected Release workflow run: event, source, attempts,
 *                every delivery job and the asset-upload and npm-publish steps
 *                that a dry run skips
 *   archives     the five platform archives and checksum files, downloaded and hashed
 *   npm          all eleven packages: exact-version metadata, tarball bytes against
 *                `dist.integrity`, the registry signature, package contents, and
 *                SLSA provenance cryptographically verified with sigstore and bound
 *                to the downloaded bytes, the repository, the tag commit, the
 *                release workflow and an eligible attempt of the selected run
 *   wasm         the four WASM files in the published @takazudo/zfb-md-wasm
 *                tarball against its dist/shipped-artifacts.json and the
 *                zfb-md-wasm-<version>-shipped-artifacts.json Release asset
 *   consumption  the release run's clean-room smoke job and the selected
 *                installer run's "Smoke released (install.sh)" job, from their logs
 *   channels     GitHub Latest, npm dist-tags and Homebrew. These are mutable and are
 *                only observed unless an --expect-* flag asks for a value.
 *
 * Read-only: every request is an HTTP GET, and the only subprocesses are
 * `git rev-parse HEAD` and `git status --porcelain` for the verifier's own
 * identity. It never publishes, uploads, dispatches or reruns workflows, edits
 * a Release, moves a dist-tag, pushes a tap or files an issue.
 *
 * Exit codes:
 *   0  every check required by the selected mode is verified
 *   1  at least one definite failure (even when other items are incomplete)
 *   2  no definite failure, but required evidence is incomplete or unavailable
 *   3  usage error or an internal error before a report could be produced
 */

import { execFileSync } from "node:child_process";
import { createHash, verify as cryptoVerify } from "node:crypto";
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";

import { PUBLISHED_PACKAGES } from "./retire-next-dist-tag.mjs";
import {
  MD_WASM_PACKAGE,
  MD_WASM_PATHS,
  assertMdWasmManifest,
  verifyMdWasmArtifacts,
} from "./verify-zfb-md-wasm-published-digests.mjs";

export const REPORT_SCHEMA_VERSION = 1;
export const DEFAULT_REPO = "Takazudo/zudo-front-builder";
export const RELEASE_WORKFLOW_PATH = ".github/workflows/release.yml";
export const INSTALLER_WORKFLOW_PATH = ".github/workflows/node-free-smoke.yml";
export const INSTALLER_JOB_NAME = "Smoke released (install.sh)";
export const GITHUB_ACTIONS_ISSUER = "https://token.actions.githubusercontent.com";
export const SLSA_PROVENANCE_V1 = "https://slsa.dev/provenance/v1";
export const GITHUB_WORKFLOW_BUILD_TYPE =
  "https://slsa-framework.github.io/github-actions-buildtypes/workflow/v1";
export const HOMEBREW_FORMULA_URL =
  "https://raw.githubusercontent.com/Takazudo/homebrew-tap/main/Formula/zfb.rb";

const GITHUB_API = "https://api.github.com";
const NPM_REGISTRY = "https://registry.npmjs.org";

export const EXIT = Object.freeze({ verified: 0, failed: 1, incomplete: 2, usage: 3 });
export const STATUS = Object.freeze({
  VERIFIED: "verified",
  FAILED: "failed",
  INCOMPLETE: "incomplete",
  NOT_APPLICABLE: "not-applicable",
  OBSERVED: "observed",
});

// The archive naming contract written by release.yml's build job and read by
// install.sh / install.ps1 / update-homebrew-formula.sh.
export const PLATFORM_TARGETS = Object.freeze([
  { target: "aarch64-apple-darwin", ext: "tar.gz" },
  { target: "x86_64-apple-darwin", ext: "tar.gz" },
  { target: "aarch64-unknown-linux-gnu", ext: "tar.gz" },
  { target: "x86_64-unknown-linux-gnu", ext: "tar.gz" },
  { target: "x86_64-pc-windows-msvc", ext: "zip" },
]);

export const PLATFORM_CARRIERS = Object.freeze({
  "@takazudo/zfb-darwin-arm64": { binary: "zfb", os: "darwin", cpu: "arm64" },
  "@takazudo/zfb-darwin-x64": { binary: "zfb", os: "darwin", cpu: "x64" },
  "@takazudo/zfb-linux-arm64-gnu": { binary: "zfb", os: "linux", cpu: "arm64" },
  "@takazudo/zfb-linux-x64-gnu": { binary: "zfb", os: "linux", cpu: "x64" },
  "@takazudo/zfb-win32-x64-msvc": { binary: "zfb.exe", os: "win32", cpu: "x64" },
});

// Job and step names of the normal provenance-enabled path in release.yml.
// `success` steps must have run and succeeded; `skipped` steps are the dry-run
// and alternative-publication branches, which must not have run.
export const RELEASE_REQUIRED_JOBS = Object.freeze([
  {
    id: "release-context",
    name: "Resolve and verify release source",
    success: ["Verify immutable source and published Release"],
  },
  ...PLATFORM_TARGETS.map(({ target, ext }) => ({
    id: `build:${target}`,
    name: `Build ${target}`,
    success: [ext === "zip" ? "Create archive (Windows zip)" : "Create archive (Unix tar.gz)"],
  })),
  {
    id: "build-wasm-md",
    name: "Build @takazudo/zfb-md-wasm npm package",
    success: ["Re-assert final @takazudo/zfb-md-wasm dist"],
  },
  {
    id: "release-assets",
    name: "Upload GitHub Release assets",
    success: [
      "Guard — verified release tag required for real release upload",
      "Upload archives and checksums to GitHub Release",
    ],
    skipped: ["DRY RUN — skip GH Release upload"],
  },
  {
    id: "publish",
    name: "publish",
    success: [
      "Verify publish context (verified release tag required)",
      "Publish to npm",
      "Verify published @takazudo/zfb-md-wasm and upload digest asset",
    ],
    skipped: [
      "DRY RUN — skip publish",
      "Publish to npm (mixed provenance — local macOS-x64)",
      "Publish to npm (recovery — no provenance)",
    ],
  },
  {
    id: "smoke-clean-room",
    namePrefix: "smoke-clean-room / ",
    success: ["Run clean-room smoke (scaffold + build + assert)"],
  },
]);

export const NORMAL_PUBLISH_STEP = "Publish to npm";
const INSTALLER_STEPS = [
  "Build smoke image (released mode)",
  "Run smoke container (released mode)",
];

// Fulcio certificate extensions (https://github.com/sigstore/fulcio/blob/main/docs/oid-info.md).
export const FULCIO_OIDS = Object.freeze({
  issuer: "1.3.6.1.4.1.57264.1.8",
  buildSignerUri: "1.3.6.1.4.1.57264.1.9",
  buildSignerDigest: "1.3.6.1.4.1.57264.1.10",
  runnerEnvironment: "1.3.6.1.4.1.57264.1.11",
  sourceRepositoryUri: "1.3.6.1.4.1.57264.1.12",
  sourceRepositoryDigest: "1.3.6.1.4.1.57264.1.13",
  sourceRepositoryRef: "1.3.6.1.4.1.57264.1.14",
  sourceRepositoryIdentifier: "1.3.6.1.4.1.57264.1.15",
  sourceRepositoryOwnerIdentifier: "1.3.6.1.4.1.57264.1.17",
  buildTrigger: "1.3.6.1.4.1.57264.1.20",
  runInvocationUri: "1.3.6.1.4.1.57264.1.21",
});

const LIMITS = Object.freeze({
  json: 32 * 1024 * 1024,
  log: 64 * 1024 * 1024,
  text: 1024 * 1024,
  archive: 256 * 1024 * 1024,
  tarball: 256 * 1024 * 1024,
  unpacked: 512 * 1024 * 1024,
  jobPages: 20,
  attempts: 20,
});

const SEMVER_RE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(?:next|beta|rc)\.(0|[1-9]\d*))?$/;

// ── Errors and HTTP ─────────────────────────────────────────────────────────

/** A classified request failure. `kind` decides how a check reports it. */
export class FetchError extends Error {
  constructor(kind, message, { url, status } = {}) {
    super(message);
    this.name = "FetchError";
    this.kind = kind;
    this.url = url;
    this.status = status;
  }
}

const RETRYABLE = new Set(["network", "timeout", "server", "rate-limit"]);

function defaultSleep(ms) {
  return new Promise((done) => setTimeout(done, ms));
}

function classifyStatus(response) {
  const { status } = response;
  if (status === 401) return "auth";
  if (status === 403) {
    if (
      response.headers.get("x-ratelimit-remaining") === "0" ||
      response.headers.get("retry-after")
    ) {
      return "rate-limit";
    }
    return "forbidden";
  }
  if (status === 429) return "rate-limit";
  if (status === 404) return "not-found";
  if (status === 410) return "gone";
  if (status >= 500) return "server";
  return "http";
}

function retryWaitMs(response, attempt, nowMs) {
  const retryAfter = response?.headers.get("retry-after");
  if (retryAfter && /^\d+$/.test(retryAfter)) return Number(retryAfter) * 1000;
  const reset = response?.headers.get("x-ratelimit-reset");
  if (response?.headers.get("x-ratelimit-remaining") === "0" && reset && /^\d+$/.test(reset)) {
    return Math.max(0, Number(reset) * 1000 - nowMs);
  }
  return Math.min(30_000, 1000 * 2 ** (attempt - 1));
}

async function readBounded(response, maxBytes, url) {
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > maxBytes) {
    throw new FetchError(
      "too-large",
      `${url}: ${declared} bytes exceeds the ${maxBytes}-byte bound`,
      { url },
    );
  }
  if (!response.body?.getReader) {
    const bytes = Buffer.from(await response.arrayBuffer());
    if (bytes.length > maxBytes) {
      throw new FetchError("too-large", `${url}: body exceeds the ${maxBytes}-byte bound`, { url });
    }
    return bytes;
  }
  const reader = response.body.getReader();
  const chunks = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > maxBytes) {
      await reader.cancel().catch(() => {});
      throw new FetchError("too-large", `${url}: body exceeds the ${maxBytes}-byte bound`, { url });
    }
    chunks.push(Buffer.from(value));
  }
  return Buffer.concat(chunks, total);
}

/**
 * GET-only HTTP client with bounded timeouts, finite retries for transient
 * answers, and classified errors. The GitHub token is sent to api.github.com only.
 */
export function createHttpClient({
  fetchImpl = globalThis.fetch,
  token,
  timeoutMs = 60_000,
  retries = 3,
  maxRetryWaitMs = 120_000,
  sleep = defaultSleep,
  now = Date.now,
} = {}) {
  const requests = [];

  async function get(url, { accept, maxBytes }) {
    const headers = { "user-agent": "zfb-verify-release-delivery" };
    if (accept) headers.accept = accept;
    const host = new URL(url).host;
    if (host === "api.github.com") {
      headers["x-github-api-version"] = "2022-11-28";
      if (token) headers.authorization = `Bearer ${token}`;
    }
    let lastError;
    for (let attempt = 1; attempt <= retries + 1; attempt += 1) {
      requests.push({ method: "GET", url, attempt });
      let response;
      try {
        response = await fetchImpl(url, {
          method: "GET",
          headers,
          redirect: "follow",
          signal: AbortSignal.timeout(timeoutMs),
        });
      } catch (error) {
        const kind =
          error?.name === "TimeoutError" || error?.name === "AbortError" ? "timeout" : "network";
        lastError = new FetchError(kind, `${url}: ${kind} error: ${error?.message ?? error}`, {
          url,
        });
        if (attempt <= retries) {
          await sleep(retryWaitMs(undefined, attempt, now()));
          continue;
        }
        throw lastError;
      }
      if (response.ok) {
        try {
          return await readBounded(response, maxBytes, url);
        } catch (error) {
          if (error instanceof FetchError) throw error;
          lastError = new FetchError("network", `${url}: body read failed: ${error.message}`, {
            url,
          });
          if (attempt <= retries) {
            await sleep(retryWaitMs(undefined, attempt, now()));
            continue;
          }
          throw lastError;
        }
      }
      const kind = classifyStatus(response);
      lastError = new FetchError(kind, `${url}: HTTP ${response.status} (${kind})`, {
        url,
        status: response.status,
      });
      await response.body?.cancel?.().catch(() => {});
      if (!RETRYABLE.has(kind) || attempt > retries) throw lastError;
      const wait = retryWaitMs(response, attempt, now());
      if (wait > maxRetryWaitMs) {
        throw new FetchError(
          kind,
          `${lastError.message}; retry wait ${wait} ms exceeds the bound`,
          {
            url,
            status: response.status,
          },
        );
      }
      await sleep(wait);
    }
    throw lastError;
  }

  return {
    requests,
    async getBytes(url, { maxBytes = LIMITS.archive, accept } = {}) {
      return get(url, { accept, maxBytes });
    },
    async getText(url, { maxBytes = LIMITS.text, accept } = {}) {
      return (await get(url, { accept, maxBytes })).toString("utf8");
    },
    async getJson(url, { maxBytes = LIMITS.json, accept = "application/json" } = {}) {
      const body = await get(url, { accept, maxBytes });
      try {
        return JSON.parse(body.toString("utf8"));
      } catch (error) {
        throw new FetchError("malformed", `${url}: response is not JSON: ${error.message}`, {
          url,
        });
      }
    },
  };
}

// ── Small helpers ───────────────────────────────────────────────────────────

export function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
}

export function npmPurl(name, version) {
  const encoded = name.startsWith("@") ? `%40${name.slice(1)}` : name;
  return `pkg:npm/${encoded}@${version}`;
}

function sha(algorithm, bytes, encoding = "hex") {
  return createHash(algorithm).update(bytes).digest(encoding);
}

function registryPath(name) {
  return name.replace("/", "%2f");
}

async function mapLimit(items, limit, fn) {
  const results = new Array(items.length);
  let next = 0;
  async function worker() {
    while (next < items.length) {
      const index = next++;
      results[index] = await fn(items[index], index);
    }
  }
  await Promise.all(Array.from({ length: Math.max(1, Math.min(limit, items.length)) }, worker));
  return results;
}

function errorRecord(error) {
  if (error instanceof FetchError) {
    return { kind: error.kind, message: error.message, url: error.url, status: error.status };
  }
  return { kind: error?.kind ?? "error", message: String(error?.message ?? error) };
}

/** A FetchError reports failed for a definitely missing object, incomplete otherwise. */
function statusForError(error, { missingIsFailure = true } = {}) {
  if (error instanceof FetchError && error.kind === "not-found" && missingIsFailure)
    return STATUS.FAILED;
  if (error?.verdict === STATUS.FAILED) return STATUS.FAILED;
  return STATUS.INCOMPLETE;
}

const RANK = { [STATUS.FAILED]: 3, [STATUS.INCOMPLETE]: 2, [STATUS.VERIFIED]: 1 };

/** Worst status wins; observed/not-applicable items do not affect the result. */
export function combineStatus(statuses) {
  let worst;
  for (const status of statuses) {
    if (!(status in RANK)) continue;
    if (!worst || RANK[status] > RANK[worst]) worst = status;
  }
  return worst ?? STATUS.NOT_APPLICABLE;
}

class Section {
  constructor(id) {
    this.id = id;
    this.checks = [];
  }
  add(id, status, evidence = {}, extra = {}) {
    const check = { id, status, evidence, ...extra };
    this.checks.push(check);
    return check;
  }
  verified(id, evidence) {
    return this.add(id, STATUS.VERIFIED, evidence);
  }
  failed(id, reason, evidence) {
    return this.add(id, STATUS.FAILED, evidence, { reason });
  }
  incomplete(id, reason, evidence) {
    return this.add(id, STATUS.INCOMPLETE, evidence, { reason });
  }
  error(id, error, evidence, options) {
    return this.add(id, statusForError(error, options), evidence, {
      reason: error.message,
      error: errorRecord(error),
    });
  }
  expect(id, condition, evidence, reason) {
    return condition ? this.verified(id, evidence) : this.failed(id, reason, evidence);
  }
  toJSON() {
    return { status: combineStatus(this.checks.map((c) => c.status)), checks: this.checks };
  }
}

// ── tar.gz reader (bounded, read-only, in memory) ───────────────────────────

function tarNumber(field) {
  if (field[0] & 0x80) {
    let value = 0n;
    for (let i = 1; i < field.length; i += 1) value = (value << 8n) | BigInt(field[i]);
    return Number(value);
  }
  const text = field.toString("ascii").replace(/\0.*$/s, "").trim();
  return text ? parseInt(text, 8) : 0;
}

function tarString(field) {
  return field.toString("utf8").replace(/\0.*$/s, "");
}

/** Return the regular files of an uncompressed tar archive. */
export function parseTar(buffer) {
  const files = new Map();
  let offset = 0;
  let longName;
  let paxPath;
  while (offset + 512 <= buffer.length) {
    const header = buffer.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) break;
    const size = tarNumber(header.subarray(124, 136));
    const type = String.fromCharCode(header[156] || 0x30);
    const dataStart = offset + 512;
    const dataEnd = dataStart + size;
    if (!Number.isSafeInteger(size) || size < 0 || dataEnd > buffer.length) {
      throw new Error(`malformed tar entry at offset ${offset}`);
    }
    const data = buffer.subarray(dataStart, dataEnd);
    if (type === "x") {
      for (const record of data.toString("utf8").split("\n")) {
        const match = /^\d+ path=(.*)$/.exec(record);
        if (match) paxPath = match[1];
      }
    } else if (type === "L") {
      longName = tarString(data);
    } else if (type === "0" || type === "\0" || type === "7") {
      const prefix = tarString(header.subarray(345, 500));
      const name = tarString(header.subarray(0, 100));
      const path = paxPath ?? longName ?? (prefix ? `${prefix}/${name}` : name);
      files.set(path, { mode: tarNumber(header.subarray(100, 108)), size, data });
      paxPath = undefined;
      longName = undefined;
    } else if (type !== "g") {
      paxPath = undefined;
      longName = undefined;
    }
    offset = dataStart + Math.ceil(size / 512) * 512;
  }
  return files;
}

export function readNpmTarball(bytes, maxUnpacked = LIMITS.unpacked) {
  let tar;
  try {
    tar = gunzipSync(bytes, { maxOutputLength: maxUnpacked });
  } catch (error) {
    const bound = error?.code === "ERR_BUFFER_TOO_LARGE" || error instanceof RangeError;
    const wrapped = new Error(
      bound
        ? `unpacked tarball exceeds the ${maxUnpacked}-byte bound`
        : `gunzip failed: ${error.message}`,
    );
    wrapped.verdict = bound ? STATUS.INCOMPLETE : STATUS.FAILED;
    throw wrapped;
  }
  const files = new Map();
  for (const [path, entry] of parseTar(tar)) {
    files.set(path.startsWith("package/") ? path.slice("package/".length) : path, entry);
  }
  return files;
}

// ── Identity of the verifier itself ─────────────────────────────────────────

function defaultRunGit(args, cwd) {
  return execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
}

function readSigstoreVersion() {
  try {
    return createRequire(import.meta.url)("sigstore/package.json").version;
  } catch {
    return undefined;
  }
}

export function controlIdentity({
  runGit = defaultRunGit,
  scriptPath = fileURLToPath(import.meta.url),
} = {}) {
  const cwd = dirname(scriptPath);
  const control = {
    script: "scripts/verify-release-delivery.mjs",
    scriptSha256: sha("sha256", readFileSync(scriptPath)),
    node: process.version,
    provenanceVerifier: `sigstore@${readSigstoreVersion() ?? "unavailable"}`,
  };
  try {
    control.commit = runGit(["rev-parse", "HEAD"], cwd).trim();
    control.dirty = runGit(["status", "--porcelain", "--untracked-files=no"], cwd).trim() !== "";
  } catch (error) {
    control.commit = null;
    control.commitError = String(error?.message ?? error);
  }
  return control;
}

// ── sigstore adapter ────────────────────────────────────────────────────────

/** Decode a Fulcio extension value: DER UTF8String (v2 OIDs) or raw bytes (v1). */
export function decodeExtensionValue(value) {
  const bytes = Buffer.from(value);
  if (bytes[0] === 0x0c && bytes.length >= 2) {
    let length = bytes[1];
    let start = 2;
    if (length & 0x80) {
      const count = length & 0x7f;
      length = 0;
      for (let i = 0; i < count; i += 1) length = (length << 8) | bytes[2 + i];
      start = 2 + count;
    }
    if (start + length === bytes.length) return bytes.subarray(start).toString("utf8");
  }
  return bytes.toString("utf8");
}

/** Map a sigstore-js Signer to plain identity fields with decoded extensions. */
export function signerIdentity(signer) {
  const oids = {};
  for (const { oid, value } of signer?.identity?.oids ?? []) {
    oids[oid.id.join(".")] = decodeExtensionValue(value);
  }
  return {
    subjectAlternativeName: signer?.identity?.subjectAlternativeName,
    issuer: signer?.identity?.extensions?.issuer,
    oids,
  };
}

/**
 * Create a bundle verifier backed by one sigstore-js Verifier: Fulcio
 * certificate chain, SCT, Rekor inclusion, timestamp and DSSE signature, plus
 * the anchored identity and exact issuer policy. The trusted root is loaded
 * once through TUF and reused for every bundle. Each call throws on failure.
 */
export function createSigstoreBundleVerifier() {
  const verifiers = new Map();
  return async function verifyBundle(bundle, { identityRegex, issuer, tufCachePath, timeoutMs }) {
    const key = JSON.stringify([identityRegex, issuer, tufCachePath]);
    if (!verifiers.has(key)) {
      verifiers.set(
        key,
        import("sigstore").then(({ createVerifier }) =>
          createVerifier({
            certificateIssuer: issuer,
            certificateIdentityURI: identityRegex,
            tufCachePath,
          }),
        ),
      );
    }
    let timer;
    const timeout = new Promise((_, reject) => {
      timer = setTimeout(
        () => reject(new FetchError("timeout", `sigstore verification exceeded ${timeoutMs} ms`)),
        timeoutMs,
      );
    });
    try {
      const verifier = await Promise.race([verifiers.get(key), timeout]);
      return signerIdentity(verifier.verify(bundle));
    } catch (error) {
      if (error instanceof FetchError) verifiers.delete(key);
      throw error;
    } finally {
      clearTimeout(timer);
    }
  };
}

// ── GitHub helpers ──────────────────────────────────────────────────────────

async function resolveTagCommit(http, repo, tag) {
  const ref = await http.getJson(`${GITHUB_API}/repos/${repo}/git/ref/tags/${tag}`);
  if (Array.isArray(ref) || ref?.ref !== `refs/tags/${tag}` || !ref.object?.sha) {
    throw new FetchError("malformed", `refs/tags/${tag}: unexpected ref response`);
  }
  let object = ref.object;
  const chain = [];
  for (let depth = 0; object.type === "tag"; depth += 1) {
    if (depth >= 5) throw new FetchError("malformed", `refs/tags/${tag}: tag chain too deep`);
    chain.push(object.sha);
    const tagObject = await http.getJson(`${GITHUB_API}/repos/${repo}/git/tags/${object.sha}`);
    if (!tagObject?.object?.sha)
      throw new FetchError("malformed", `tag object ${object.sha} is malformed`);
    object = tagObject.object;
  }
  if (object.type !== "commit" || !/^[0-9a-f]{40}$/.test(object.sha)) {
    throw new FetchError("malformed", `refs/tags/${tag} does not peel to a commit`);
  }
  return { commit: object.sha, annotated: chain.length > 0, tagObjects: chain };
}

/** Fetch every job of every attempt of a run, following pagination to the end. */
export async function listAllJobs(http, repo, runId) {
  const jobs = [];
  let total;
  for (let page = 1; page <= LIMITS.jobPages; page += 1) {
    const body = await http.getJson(
      `${GITHUB_API}/repos/${repo}/actions/runs/${runId}/jobs?filter=all&per_page=100&page=${page}`,
    );
    if (!Array.isArray(body?.jobs) || !Number.isSafeInteger(body?.total_count)) {
      throw new FetchError("malformed", `run ${runId} jobs page ${page} is malformed`);
    }
    total = body.total_count;
    jobs.push(...body.jobs);
    if (body.jobs.length === 0 || jobs.length >= total) break;
  }
  const unique = [...new Map(jobs.map((job) => [job.id, job])).values()];
  if (total === undefined || unique.length !== total) {
    throw new FetchError(
      "malformed",
      `run ${runId}: collected ${unique.length} jobs but the API reports ${total} (pagination incomplete)`,
    );
  }
  return unique;
}

/**
 * Group jobs by name. The effective execution of a job is its highest attempt:
 * a partial rerun carries successful jobs forward with their original attempt
 * number, and a later failure always outranks an earlier success.
 */
export function effectiveJobs(jobs) {
  const byName = new Map();
  for (const job of jobs) {
    const list = byName.get(job.name) ?? [];
    list.push(job);
    byName.set(job.name, list);
  }
  const result = new Map();
  for (const [name, list] of byName) {
    list.sort((a, b) => (a.run_attempt ?? 1) - (b.run_attempt ?? 1) || a.id - b.id);
    const effective = list.at(-1);
    const sameAttempt = list.filter(
      (job) => (job.run_attempt ?? 1) === (effective.run_attempt ?? 1),
    );
    result.set(name, { effective, history: list, duplicates: sameAttempt.length });
  }
  return result;
}

function jobSummary(job) {
  return {
    id: job.id,
    name: job.name,
    attempt: job.run_attempt ?? 1,
    status: job.status,
    conclusion: job.conclusion,
    html_url: job.html_url,
  };
}

function findStep(job, name) {
  return (job?.steps ?? []).find((step) => step.name === name);
}

/** Validate one required job's effective execution and its delivery steps. */
function checkRequiredJob(section, grouped, spec, prefix) {
  const id = `${prefix}:${spec.id}`;
  const matches = spec.namePrefix
    ? [...grouped.entries()].filter(([name]) => name.startsWith(spec.namePrefix)).map(([, e]) => e)
    : [grouped.get(spec.name)].filter(Boolean);
  if (matches.length === 0) {
    section.failed(id, `required job "${spec.name ?? spec.namePrefix}" is absent from the run`);
    return undefined;
  }
  if (matches.length > 1 || matches[0].duplicates > 1) {
    section.failed(
      id,
      `required job "${spec.name ?? spec.namePrefix}" is ambiguous: several jobs match in one attempt`,
      {
        jobs: matches.flatMap((e) => e.history.map(jobSummary)),
      },
    );
    return undefined;
  }
  const [entry] = matches;
  const job = entry.effective;
  const evidence = {
    effective: jobSummary(job),
    history: entry.history.map(jobSummary),
    basis: "highest attempt of this job name",
  };
  if (job.status !== "completed") {
    section.incomplete(id, `job "${job.name}" is ${job.status}`, evidence);
    return job;
  }
  if (job.conclusion !== "success") {
    section.failed(id, `job "${job.name}" concluded ${job.conclusion}`, evidence);
    return job;
  }
  const problems = [];
  const steps = {};
  for (const name of spec.success ?? []) {
    const step = findStep(job, name);
    steps[name] = step ? `${step.status}/${step.conclusion}` : "absent";
    if (!step) problems.push(`required step "${name}" is absent`);
    else if (step.conclusion === "skipped") problems.push(`required step "${name}" was skipped`);
    else if (step.conclusion !== "success")
      problems.push(`required step "${name}" concluded ${step.conclusion}`);
  }
  for (const name of spec.skipped ?? []) {
    const step = findStep(job, name);
    steps[name] = step ? `${step.status}/${step.conclusion}` : "absent";
    if (step && step.conclusion !== "skipped") {
      problems.push(`step "${name}" ran (${step.conclusion}); the normal delivery path skips it`);
    }
  }
  evidence.steps = steps;
  if (problems.length) section.failed(id, problems.join("; "), evidence);
  else section.verified(id, evidence);
  return job;
}

/** Classify which branch of release.yml a run took. */
export function classifyDeliveryPath(run, grouped) {
  const publish = grouped.get("publish")?.history ?? [];
  const assets = grouped.get("Upload GitHub Release assets")?.history ?? [];
  const ran = (jobs, step) =>
    jobs.some((job) => {
      const found = findStep(job, step);
      return found && found.conclusion && found.conclusion !== "skipped";
    });
  if (ran(publish, "DRY RUN — skip publish") || ran(assets, "DRY RUN — skip GH Release upload")) {
    return "dry-run";
  }
  if (ran(publish, "Publish to npm (recovery — no provenance)")) return "recovery";
  if (ran(publish, "Publish to npm (mixed provenance — local macOS-x64)"))
    return "mixed-provenance";
  if (run.event === "workflow_dispatch") return "manual-dispatch";
  if (run.event !== "release") return `unsupported-event:${run.event}`;
  if (!grouped.has("Build x86_64-apple-darwin")) return "mixed-provenance";
  return "normal";
}

function cleanLog(text) {
  return text
    .replace(/^﻿/, "")
    .split(/\r?\n/)
    .map((line) => line.replace(/^\d{4}-\d\d-\d\dT[\d:.]+Z ?/, "").replace(/\x1b\[[0-9;]*m/g, ""));
}

function logEvidence(lines, expectations) {
  const found = {};
  const missing = [];
  for (const [key, pattern] of Object.entries(expectations)) {
    const line = lines.find((candidate) =>
      pattern instanceof RegExp ? pattern.test(candidate) : candidate.includes(pattern),
    );
    if (line === undefined) missing.push(key);
    else found[key] = line.trim().slice(0, 300);
  }
  return { found, missing };
}

// ── Sections ────────────────────────────────────────────────────────────────

async function verifyIdentity(ctx) {
  const section = new Section("identity");
  const { http, repo, tag, version } = ctx;
  try {
    const resolved = await resolveTagCommit(http, repo, tag);
    ctx.commit = resolved.commit;
    section.verified("tag", { tag, ...resolved });
  } catch (error) {
    section.error("tag", error, { tag });
  }
  if (ctx.expectedSource && ctx.commit) {
    section.expect(
      "expected-source",
      ctx.commit === ctx.expectedSource,
      { expected: ctx.expectedSource, actual: ctx.commit },
      `tag ${tag} resolves to ${ctx.commit}, not the expected ${ctx.expectedSource}`,
    );
  }
  try {
    const release = await http.getJson(`${GITHUB_API}/repos/${repo}/releases/tags/${tag}`);
    ctx.release = release;
    const evidence = {
      id: release.id,
      tag_name: release.tag_name,
      draft: release.draft,
      prerelease: release.prerelease,
      published_at: release.published_at,
      target_commitish: release.target_commitish,
      immutable: release.immutable,
      html_url: release.html_url,
    };
    const problems = [];
    if (release.tag_name !== tag) problems.push(`tag_name is ${release.tag_name}`);
    if (release.draft !== false) problems.push("Release is a draft");
    if (!release.published_at) problems.push("Release has no published_at");
    if (release.prerelease !== version.includes("-")) {
      problems.push(`prerelease=${release.prerelease} does not match version ${version}`);
    }
    if (/^[0-9a-f]{40}$/i.test(release.target_commitish ?? "")) {
      if (ctx.commit && release.target_commitish.toLowerCase() !== ctx.commit) {
        problems.push(`target_commitish ${release.target_commitish} differs from the tag commit`);
      }
    } else {
      evidence.note = "target_commitish is a branch name; the tag commit is the immutable source";
    }
    if (problems.length) section.failed("release", problems.join("; "), evidence);
    else section.verified("release", evidence);
  } catch (error) {
    section.error("release", error, { tag });
  }
  if (ctx.commit) {
    try {
      const file = await http.getJson(
        `${GITHUB_API}/repos/${repo}/contents/packages/zfb/package.json?ref=${ctx.commit}`,
      );
      const manifest = JSON.parse(Buffer.from(file.content ?? "", "base64").toString("utf8"));
      section.expect(
        "source-version",
        manifest.version === version,
        { path: "packages/zfb/package.json", ref: ctx.commit, version: manifest.version },
        `packages/zfb/package.json at ${ctx.commit} is ${manifest.version}, not ${version}`,
      );
    } catch (error) {
      section.error(
        "source-version",
        error instanceof SyntaxError ? new FetchError("malformed", error.message) : error,
      );
    }
  }
  return section;
}

async function verifyReleaseWorkflow(ctx) {
  const section = new Section("workflow");
  const { http, repo, tag } = ctx;
  let run;
  try {
    run = await http.getJson(`${GITHUB_API}/repos/${repo}/actions/runs/${ctx.releaseRunId}`);
  } catch (error) {
    section.error("release-run", error, { runId: ctx.releaseRunId });
    return section;
  }
  ctx.releaseRun = run;
  if (run.repository?.full_name?.toLowerCase() === repo.toLowerCase()) {
    ctx.canonicalRepo = run.repository.full_name;
    ctx.repositoryId = run.repository.id;
    ctx.ownerId = run.repository.owner?.id;
  }
  const runEvidence = {
    id: run.id,
    path: run.path,
    event: run.event,
    head_branch: run.head_branch,
    head_sha: run.head_sha,
    run_attempt: run.run_attempt,
    status: run.status,
    conclusion: run.conclusion,
    html_url: run.html_url,
  };
  const problems = [];
  if (run.path !== RELEASE_WORKFLOW_PATH) problems.push(`workflow path is ${run.path}`);
  if (run.repository?.full_name && run.repository.full_name.toLowerCase() !== repo.toLowerCase()) {
    problems.push(`run belongs to ${run.repository.full_name}`);
  }
  if (run.head_branch !== tag) problems.push(`head_branch is ${run.head_branch}, not ${tag}`);
  if (ctx.commit && run.head_sha !== ctx.commit)
    problems.push(`head_sha ${run.head_sha} is not the tag commit`);
  if (problems.length) section.failed("release-run", problems.join("; "), runEvidence);
  else if (run.status !== "completed")
    section.incomplete("release-run", `run is ${run.status}`, runEvidence);
  else if (run.conclusion !== "success") {
    section.failed("release-run", `latest attempt concluded ${run.conclusion}`, runEvidence);
  } else section.verified("release-run", runEvidence);

  const attempts = [];
  const lastAttempt = Math.min(run.run_attempt ?? 1, LIMITS.attempts);
  for (let attempt = 1; attempt < lastAttempt; attempt += 1) {
    try {
      const prior = await http.getJson(
        `${GITHUB_API}/repos/${repo}/actions/runs/${run.id}/attempts/${attempt}`,
      );
      attempts.push({ attempt, status: prior.status, conclusion: prior.conclusion });
    } catch (error) {
      attempts.push({ attempt, error: errorRecord(error) });
    }
  }
  attempts.push({ attempt: run.run_attempt ?? 1, status: run.status, conclusion: run.conclusion });
  section.add("attempts", STATUS.OBSERVED, { attempts });

  let jobs;
  try {
    jobs = await listAllJobs(http, repo, run.id);
  } catch (error) {
    section.error("jobs", error, { runId: run.id }, { missingIsFailure: false });
    return section;
  }
  const grouped = effectiveJobs(jobs);
  ctx.releaseJobs = grouped;
  const path = classifyDeliveryPath(run, grouped);
  ctx.deliveryPath = path;
  if (path === "normal") section.verified("delivery-path", { path });
  else if (path === "dry-run") {
    section.failed("delivery-path", "the run took the dry-run branch and delivered nothing", {
      path,
    });
  } else {
    section.incomplete(
      "delivery-path",
      `delivery path "${path}" is not supported by this verifier (normal provenance-enabled release only)`,
      { path },
    );
  }
  for (const spec of RELEASE_REQUIRED_JOBS) {
    const job = checkRequiredJob(section, grouped, spec, "job");
    if (job && job.run_attempt > (run.run_attempt ?? 1)) {
      section.failed(
        `job:${spec.id}:attempt`,
        `job attempt ${job.run_attempt} exceeds the run attempt`,
      );
    }
  }
  const publishHistory = grouped.get("publish")?.history ?? [];
  ctx.eligibleAttempts = publishHistory
    .filter((job) => {
      const step = findStep(job, NORMAL_PUBLISH_STEP);
      return step && (step.conclusion === "success" || step.conclusion === "failure");
    })
    .map((job) => job.run_attempt ?? 1);
  section.add("eligible-publish-attempts", STATUS.OBSERVED, {
    attempts: ctx.eligibleAttempts,
    basis: `attempts of run ${run.id} whose "${NORMAL_PUBLISH_STEP}" step executed`,
  });
  return section;
}

async function verifyArchives(ctx) {
  const section = new Section("archives");
  const { http, version, release } = ctx;
  if (!release) {
    section.incomplete("release-assets", "the Release could not be read");
    return section;
  }
  const assets = release.assets ?? [];
  const expectedNames = new Set([`zfb-md-wasm-${version}-shipped-artifacts.json`]);
  await mapLimit(PLATFORM_TARGETS, ctx.concurrency, async ({ target, ext }) => {
    const name = `zfb-${version}-${target}.${ext}`;
    const checksumName = `${name}.sha256`;
    expectedNames.add(name).add(checksumName);
    const archives = assets.filter((asset) => asset.name === name);
    const checksums = assets.filter((asset) => asset.name === checksumName);
    const id = `archive:${target}`;
    if (archives.length !== 1 || checksums.length !== 1) {
      section.failed(
        id,
        `expected one ${name} and one ${checksumName}; found ${archives.length} and ${checksums.length}`,
      );
      return;
    }
    const [archive] = archives;
    const [checksum] = checksums;
    const evidence = {
      archive: {
        id: archive.id,
        name,
        url: archive.browser_download_url,
        size: archive.size,
        state: archive.state,
        apiDigest: archive.digest,
      },
      checksum: {
        id: checksum.id,
        name: checksumName,
        size: checksum.size,
        apiDigest: checksum.digest,
      },
    };
    try {
      const [archiveBytes, checksumBytes] = await Promise.all([
        http.getBytes(archive.browser_download_url, {
          maxBytes: LIMITS.archive,
          accept: "application/octet-stream",
        }),
        http.getBytes(checksum.browser_download_url, {
          maxBytes: 4096,
          accept: "application/octet-stream",
        }),
      ]);
      const computed = sha("sha256", archiveBytes);
      const text = checksumBytes.toString("utf8");
      const match = /^([0-9a-f]{64}) {2}(\S+)\r?\n?$/.exec(text);
      evidence.archive.downloadedBytes = archiveBytes.length;
      evidence.archive.computedSha256 = computed;
      evidence.checksum.content = text.trim();
      const problems = [];
      if (archive.state !== "uploaded" || checksum.state !== "uploaded")
        problems.push("asset state is not uploaded");
      if (archiveBytes.length !== archive.size)
        problems.push(`downloaded ${archiveBytes.length} bytes, asset size ${archive.size}`);
      const magicOk =
        ext === "zip"
          ? archiveBytes.subarray(0, 4).equals(Buffer.from([0x50, 0x4b, 0x03, 0x04]))
          : archiveBytes[0] === 0x1f && archiveBytes[1] === 0x8b;
      if (!magicOk) problems.push(`archive bytes are not a ${ext} file`);
      if (!match) problems.push("checksum file is not one GNU sha256sum line");
      else {
        evidence.checksum.expectedSha256 = match[1];
        if (match[2] !== name) problems.push(`checksum file names ${match[2]}, not ${name}`);
        if (match[1] !== computed) problems.push("archive SHA-256 differs from its checksum file");
      }
      if (archive.digest && archive.digest !== `sha256:${computed}`) {
        problems.push(`API digest ${archive.digest} differs from the downloaded bytes`);
      }
      if (problems.length) section.failed(id, problems.join("; "), evidence);
      else section.verified(id, evidence);
    } catch (error) {
      section.error(id, error, evidence);
    }
  });
  const unexpected = assets.map((asset) => asset.name).filter((name) => !expectedNames.has(name));
  section.add("asset-inventory", STATUS.OBSERVED, { count: assets.length, unexpected });
  return section;
}

async function loadRegistryKeys(ctx) {
  ctx.registryKeys ??= ctx.http.getJson(`${NPM_REGISTRY}/-/npm/v1/keys`).then((body) => {
    if (!Array.isArray(body?.keys))
      throw new FetchError("malformed", "npm registry keys are malformed");
    return body.keys;
  });
  return ctx.registryKeys;
}

function verifyRegistrySignatures(name, version, integrity, signatures, keys) {
  const data = Buffer.from(`${name}@${version}:${integrity}`);
  const results = (signatures ?? []).map(({ keyid, sig }) => {
    const key = keys.find((candidate) => candidate.keyid === keyid);
    if (!key) return { keyid, verified: false, reason: "unknown keyid" };
    let verified = false;
    try {
      verified = cryptoVerify(
        "sha256",
        data,
        { key: Buffer.from(key.key, "base64"), format: "der", type: "spki" },
        Buffer.from(sig, "base64"),
      );
    } catch (error) {
      return { keyid, verified: false, reason: error.message };
    }
    return { keyid, verified, expires: key.expires ?? null };
  });
  return results;
}

/** Check a decoded in-toto statement and the verified certificate identity. */
export function checkProvenanceClaims({ statement, signer, expected }) {
  const problems = [];
  const subject = statement?.subject;
  if (statement?._type !== "https://in-toto.io/Statement/v1")
    problems.push("statement _type is not in-toto v1");
  if (statement?.predicateType !== SLSA_PROVENANCE_V1)
    problems.push("predicateType is not SLSA provenance v1");
  if (!Array.isArray(subject) || subject.length !== 1)
    problems.push("statement must have exactly one subject");
  const [first] = subject ?? [];
  if (first?.name !== expected.purl)
    problems.push(`subject name ${first?.name} is not ${expected.purl}`);
  if (first?.digest?.sha512 !== expected.sha512)
    problems.push("subject sha512 differs from the downloaded tarball");
  const predicate = statement?.predicate ?? {};
  const build = predicate.buildDefinition ?? {};
  const workflow = build.externalParameters?.workflow ?? {};
  if (build.buildType !== GITHUB_WORKFLOW_BUILD_TYPE)
    problems.push(`buildType is ${build.buildType}`);
  if (workflow.repository !== expected.repositoryUri)
    problems.push(`workflow repository is ${workflow.repository}`);
  if (workflow.path !== RELEASE_WORKFLOW_PATH) problems.push(`workflow path is ${workflow.path}`);
  if (workflow.ref !== expected.ref) problems.push(`workflow ref is ${workflow.ref}`);
  if (build.internalParameters?.github?.event_name !== "release") {
    problems.push(`event_name is ${build.internalParameters?.github?.event_name}`);
  }
  const deps = build.resolvedDependencies ?? [];
  const source = deps.find((dep) => dep?.digest?.gitCommit);
  if (deps.length !== 1 || source?.digest?.gitCommit !== expected.commit) {
    problems.push(`resolved source commit is ${source?.digest?.gitCommit}, not ${expected.commit}`);
  }
  if (source && source.uri !== `git+${expected.repositoryUri}@${expected.ref}`) {
    problems.push(`resolved source uri is ${source.uri}`);
  }
  const invocationId = predicate.runDetails?.metadata?.invocationId;
  const invocation =
    /^https:\/\/github\.com\/([^/]+\/[^/]+)\/actions\/runs\/(\d+)\/attempts\/(\d+)$/.exec(
      invocationId ?? "",
    );
  let attempt;
  if (!invocation) problems.push(`invocationId ${invocationId} is not a GitHub run attempt URL`);
  else {
    attempt = Number(invocation[3]);
    if (`https://github.com/${invocation[1]}` !== expected.repositoryUri) {
      problems.push(`invocation repository is ${invocation[1]}`);
    }
    if (invocation[2] !== String(expected.runId))
      problems.push(`invocation run ${invocation[2]} is not ${expected.runId}`);
    if (!expected.eligibleAttempts.includes(attempt)) {
      problems.push(
        `invocation attempt ${attempt} is not an eligible publish attempt (${expected.eligibleAttempts.join(",") || "none"})`,
      );
    }
  }
  const oids = signer?.oids ?? {};
  const wantOids = {
    [FULCIO_OIDS.issuer]: GITHUB_ACTIONS_ISSUER,
    [FULCIO_OIDS.buildSignerUri]: expected.identity,
    [FULCIO_OIDS.buildSignerDigest]: expected.commit,
    [FULCIO_OIDS.runnerEnvironment]: "github-hosted",
    [FULCIO_OIDS.sourceRepositoryUri]: expected.repositoryUri,
    [FULCIO_OIDS.sourceRepositoryDigest]: expected.commit,
    [FULCIO_OIDS.sourceRepositoryRef]: expected.ref,
    [FULCIO_OIDS.buildTrigger]: "release",
    [FULCIO_OIDS.runInvocationUri]: invocationId,
  };
  // Numeric ids pin the repository and owner against a rename-and-recreate.
  if (expected.repositoryId != null)
    wantOids[FULCIO_OIDS.sourceRepositoryIdentifier] = String(expected.repositoryId);
  if (expected.ownerId != null)
    wantOids[FULCIO_OIDS.sourceRepositoryOwnerIdentifier] = String(expected.ownerId);
  for (const [oid, value] of Object.entries(wantOids)) {
    if (oids[oid] !== value)
      problems.push(`certificate extension ${oid} is ${oids[oid]}, expected ${value}`);
  }
  if (signer?.subjectAlternativeName !== expected.identity) {
    problems.push(`certificate SAN is ${signer?.subjectAlternativeName}`);
  }
  if (signer?.issuer !== GITHUB_ACTIONS_ISSUER)
    problems.push(`certificate issuer is ${signer?.issuer}`);
  return { problems, invocationId, attempt };
}

async function verifyProvenance(ctx, name, record, tarballSha512) {
  const result = {
    attestationPresent: false,
    signatureVerified: false,
    identityMatched: false,
    subjectBound: false,
    tool: ctx.control.provenanceVerifier,
  };
  const attestations = record.dist?.attestations;
  if (!attestations?.url || attestations.provenance?.predicateType !== SLSA_PROVENANCE_V1) {
    return {
      status: STATUS.FAILED,
      reason: "no SLSA provenance attestation is advertised",
      result,
    };
  }
  let body;
  try {
    body = await ctx.http.getJson(attestations.url);
  } catch (error) {
    return {
      status: statusForError(error),
      reason: error.message,
      error: errorRecord(error),
      result,
    };
  }
  const entries = (body?.attestations ?? []).filter(
    (entry) => entry?.predicateType === SLSA_PROVENANCE_V1,
  );
  result.publishAttestationPresent = (body?.attestations ?? []).some((entry) =>
    String(entry?.predicateType).startsWith(
      "https://github.com/npm/attestation/tree/main/specs/publish/",
    ),
  );
  if (entries.length !== 1) {
    return {
      status: STATUS.FAILED,
      reason: `expected one SLSA provenance bundle, found ${entries.length}`,
      result,
    };
  }
  result.attestationPresent = true;
  const { bundle } = entries[0];
  const identity = `https://github.com/${ctx.canonicalRepo}/${RELEASE_WORKFLOW_PATH}@refs/tags/${ctx.tag}`;
  result.policy = {
    identity,
    identityRegex: `^${escapeRegExp(identity)}$`,
    issuer: GITHUB_ACTIONS_ISSUER,
  };
  if (!ctx.verifyBundle) {
    return {
      status: STATUS.INCOMPLETE,
      reason: "no cryptographic provenance verifier is available",
      result,
    };
  }
  let signer;
  try {
    signer = await ctx.verifyBundle(bundle, {
      identityRegex: result.policy.identityRegex,
      issuer: GITHUB_ACTIONS_ISSUER,
      tufCachePath: ctx.tufCachePath,
      timeoutMs: ctx.timeoutMs,
    });
    result.signatureVerified = true;
    result.signer = {
      subjectAlternativeName: signer.subjectAlternativeName,
      issuer: signer.issuer,
    };
  } catch (error) {
    const transport =
      error instanceof FetchError ||
      ["ENOTFOUND", "ECONNRESET", "ETIMEDOUT", "ECONNREFUSED"].includes(error?.code) ||
      /TUF|fetch|network|timeout/i.test(error?.code ?? "");
    return {
      status: transport ? STATUS.INCOMPLETE : STATUS.FAILED,
      reason:
        `sigstore verification ${transport ? "unavailable" : "failed"}: ${error?.code ?? ""} ${error?.message ?? error}`.trim(),
      error: errorRecord(error),
      result,
    };
  }
  let statement;
  try {
    if (bundle?.dsseEnvelope?.payloadType !== "application/vnd.in-toto+json") {
      throw new Error(`payloadType is ${bundle?.dsseEnvelope?.payloadType}`);
    }
    statement = JSON.parse(Buffer.from(bundle.dsseEnvelope.payload, "base64").toString("utf8"));
  } catch (error) {
    return {
      status: STATUS.FAILED,
      reason: `provenance payload is malformed: ${error.message}`,
      result,
    };
  }
  if (!ctx.commit || !ctx.eligibleAttempts) {
    return {
      status: STATUS.INCOMPLETE,
      reason:
        "signature verified, but the tag commit or the release run's publish attempts are unresolved",
      result,
    };
  }
  const expected = {
    purl: npmPurl(name, ctx.version),
    sha512: tarballSha512,
    repositoryUri: `https://github.com/${ctx.canonicalRepo}`,
    ref: `refs/tags/${ctx.tag}`,
    commit: ctx.commit,
    runId: ctx.releaseRunId,
    eligibleAttempts: ctx.eligibleAttempts ?? [],
    repositoryId: ctx.repositoryId,
    ownerId: ctx.ownerId,
    identity,
  };
  const claims = checkProvenanceClaims({ statement, signer, expected });
  result.subject = statement.subject;
  result.invocationId = claims.invocationId;
  result.attempt = claims.attempt;
  result.sourceCommit =
    statement.predicate?.buildDefinition?.resolvedDependencies?.[0]?.digest?.gitCommit;
  result.subjectBound = !claims.problems.some((p) => p.startsWith("subject"));
  result.identityMatched = claims.problems.length === 0;
  if (claims.problems.length)
    return { status: STATUS.FAILED, reason: claims.problems.join("; "), result };
  return { status: STATUS.VERIFIED, result };
}

async function verifyNpmPackage(ctx, name) {
  const { http, version } = ctx;
  const pkg = { name, version, checks: {} };
  const mark = (id, status, detail = {}) => {
    pkg.checks[id] = { status, ...detail };
  };
  let record;
  try {
    record = await http.getJson(`${NPM_REGISTRY}/${registryPath(name)}/${version}`);
  } catch (error) {
    mark("metadata", statusForError(error), { reason: error.message, error: errorRecord(error) });
    return finishPackage(pkg);
  }
  const integrity = record.dist?.integrity;
  pkg.tarball = record.dist?.tarball;
  pkg.integrity = integrity;
  if (
    record.name !== name ||
    record.version !== version ||
    !pkg.tarball ||
    !/^sha512-/.test(integrity ?? "")
  ) {
    mark("metadata", STATUS.FAILED, {
      reason: "registry metadata name/version/tarball/integrity is wrong or missing",
    });
    return finishPackage(pkg);
  }
  mark("metadata", STATUS.VERIFIED);

  let bytes;
  try {
    bytes = await http.getBytes(pkg.tarball, { maxBytes: LIMITS.tarball });
  } catch (error) {
    mark("tarball", statusForError(error), { reason: error.message, error: errorRecord(error) });
    return finishPackage(pkg);
  }
  const sha512 = sha("sha512", bytes);
  pkg.computed = {
    bytes: bytes.length,
    sha512: `sha512-${Buffer.from(sha512, "hex").toString("base64")}`,
    sha1: sha("sha1", bytes),
  };
  const integrityOk =
    pkg.computed.sha512 === integrity &&
    (!record.dist.shasum || record.dist.shasum === pkg.computed.sha1);
  mark(
    "tarball",
    integrityOk ? STATUS.VERIFIED : STATUS.FAILED,
    integrityOk ? {} : { reason: "tarball bytes do not match dist.integrity/shasum" },
  );

  try {
    const keys = await loadRegistryKeys(ctx);
    const signatures = verifyRegistrySignatures(
      name,
      version,
      integrity,
      record.dist.signatures,
      keys,
    );
    const ok = signatures.length > 0 && signatures.every((entry) => entry.verified);
    mark("registry-signature", ok ? STATUS.VERIFIED : STATUS.FAILED, {
      signatures,
      ...(ok ? {} : { reason: "registry signature missing or invalid" }),
    });
  } catch (error) {
    mark("registry-signature", STATUS.INCOMPLETE, {
      reason: error.message,
      error: errorRecord(error),
    });
  }

  if (integrityOk) {
    try {
      const files = readNpmTarball(bytes);
      const manifest = JSON.parse(files.get("package.json")?.data.toString("utf8") ?? "null");
      const problems = [];
      if (manifest?.name !== name || manifest?.version !== version)
        problems.push("packed package.json name/version differs");
      const carrier = PLATFORM_CARRIERS[name];
      const contents = { fileCount: files.size };
      if (carrier) {
        const binary = files.get(carrier.binary);
        contents.binary = binary
          ? { path: carrier.binary, bytes: binary.size, mode: binary.mode.toString(8) }
          : null;
        if (!binary || binary.size === 0)
          problems.push(`carrier binary ${carrier.binary} is missing or empty`);
        else if (carrier.os !== "win32" && (binary.mode & 0o111) === 0)
          problems.push(`${carrier.binary} is not executable`);
        if (!files.get("zfb-css.node")?.size) problems.push("zfb-css.node is missing");
        if (
          JSON.stringify(manifest?.os) !== JSON.stringify([carrier.os]) ||
          JSON.stringify(manifest?.cpu) !== JSON.stringify([carrier.cpu])
        ) {
          problems.push(
            `os/cpu is ${JSON.stringify(manifest?.os)}/${JSON.stringify(manifest?.cpu)}`,
          );
        }
      }
      if (name === MD_WASM_PACKAGE) ctx.mdWasmFiles = files;
      pkg.contents = contents;
      mark(
        "contents",
        problems.length ? STATUS.FAILED : STATUS.VERIFIED,
        problems.length ? { reason: problems.join("; ") } : {},
      );
    } catch (error) {
      mark("contents", error.verdict ?? STATUS.FAILED, { reason: error.message });
    }
  } else {
    mark("contents", STATUS.FAILED, { reason: "not inspected: tarball integrity failed" });
  }

  const provenance = await verifyProvenance(ctx, name, record, sha512);
  pkg.provenance = provenance.result;
  mark(
    "provenance",
    provenance.status,
    provenance.reason ? { reason: provenance.reason, error: provenance.error } : {},
  );
  return finishPackage(pkg);
}

function finishPackage(pkg) {
  pkg.status = combineStatus(Object.values(pkg.checks).map((check) => check.status));
  return pkg;
}

async function verifyNpm(ctx) {
  const section = new Section("npm");
  const packages = await mapLimit(PUBLISHED_PACKAGES, ctx.concurrency, (name) =>
    verifyNpmPackage(ctx, name),
  );
  for (const pkg of packages) {
    const failing = Object.entries(pkg.checks)
      .filter(([, check]) => check.status !== STATUS.VERIFIED)
      .map(([id, check]) => `${id}: ${check.reason ?? check.status}`);
    section.add(
      `package:${pkg.name}`,
      pkg.status,
      pkg,
      failing.length ? { reason: failing.join("; ") } : {},
    );
  }
  section.add("coverage", PUBLISHED_PACKAGES.length === 11 ? STATUS.VERIFIED : STATUS.FAILED, {
    expected: PUBLISHED_PACKAGES,
    checked: packages.map((pkg) => pkg.name),
  });
  return section;
}

async function verifyWasm(ctx) {
  const section = new Section("wasm");
  const files = ctx.mdWasmFiles;
  if (!files) {
    section.incomplete(
      "packed-artifacts",
      `${MD_WASM_PACKAGE}@${ctx.version} tarball bytes were not verified`,
    );
    return section;
  }
  const manifestEntry = files.get("dist/shipped-artifacts.json");
  let manifest;
  try {
    if (!manifestEntry) throw new Error("dist/shipped-artifacts.json is missing from the tarball");
    manifest = JSON.parse(manifestEntry.data.toString("utf8"));
    assertMdWasmManifest(manifest, ctx.version);
    const results = verifyMdWasmArtifacts(manifest, (path) => {
      const entry = files.get(path);
      if (!entry) throw new Error(`${path} is missing from the tarball`);
      return entry.data;
    });
    section.verified("packed-artifacts", {
      artifacts: results,
      expectedPaths: MD_WASM_PATHS.map(([, path]) => path),
    });
  } catch (error) {
    section.failed("packed-artifacts", error.message);
  }
  const assetName = `zfb-md-wasm-${ctx.version}-shipped-artifacts.json`;
  const asset = (ctx.release?.assets ?? []).find((candidate) => candidate.name === assetName);
  if (!ctx.release) section.incomplete("release-manifest", "the Release could not be read");
  else if (!asset) section.failed("release-manifest", `Release asset ${assetName} is missing`);
  else {
    try {
      const bytes = await ctx.http.getBytes(asset.browser_download_url, { maxBytes: LIMITS.text });
      const evidence = {
        assetId: asset.id,
        name: assetName,
        size: asset.size,
        sha256: sha("sha256", bytes),
      };
      if (!manifestEntry)
        section.failed("release-manifest", "no packed manifest to compare", evidence);
      else {
        section.expect(
          "release-manifest",
          bytes.equals(manifestEntry.data),
          evidence,
          "Release manifest asset differs from the packed dist/shipped-artifacts.json",
        );
      }
    } catch (error) {
      section.error("release-manifest", error, { assetId: asset.id });
    }
  }
  return section;
}

async function readJobLog(ctx, repo, job) {
  return cleanLog(
    await ctx.http.getText(`${GITHUB_API}/repos/${repo}/actions/jobs/${job.id}/logs`, {
      maxBytes: LIMITS.log,
    }),
  );
}

async function verifyConsumption(ctx) {
  const section = new Section("consumption");
  const { http, repo, version, tag } = ctx;
  const smoke = ctx.releaseJobs
    ? [...ctx.releaseJobs.entries()].find(([name]) => name.startsWith("smoke-clean-room / "))?.[1]
        ?.effective
    : undefined;
  if (!smoke)
    section.incomplete(
      "release-clean-room",
      "release run jobs are unavailable or lack the clean-room job",
    );
  else if (smoke.status !== "completed" || smoke.conclusion !== "success") {
    section.add(
      "release-clean-room",
      smoke.status === "completed" ? STATUS.FAILED : STATUS.INCOMPLETE,
      jobSummary(smoke),
      {
        reason: `clean-room job is ${smoke.status}/${smoke.conclusion}`,
      },
    );
  } else {
    if (!ctx.commit) {
      section.incomplete(
        "release-clean-room",
        "the tag commit is unresolved, so the log cannot be bound to it",
      );
    } else
      try {
        const lines = await readJobLog(ctx, repo, smoke);
        const { found, missing } = logEvidence(lines, {
          reusableWorkflowAtTag: `reusable-smoke-clean-room.yml@refs/tags/${tag} (${ctx.commit})`,
          expectedVersionInput: new RegExp(`^\\s*expected_version: ${escapeRegExp(version)}$`),
          registryResolved: new RegExp(
            `Registry resolved create-zfb@\\S+ -> ${escapeRegExp(version)}$`,
          ),
          installedZfb: new RegExp(`^\\+ @takazudo/zfb ${escapeRegExp(version)}$`),
          installedRuntime: new RegExp(`^\\+ @takazudo/zfb-runtime ${escapeRegExp(version)}$`),
          builtSite: "[PASS] dist/index.html exists",
        });
        const evidence = { job: jobSummary(smoke), found };
        if (missing.length)
          section.failed(
            "release-clean-room",
            `log lacks exact-version evidence: ${missing.join(", ")}`,
            evidence,
          );
        else section.verified("release-clean-room", evidence);
      } catch (error) {
        section.error(
          "release-clean-room",
          error,
          { job: jobSummary(smoke) },
          { missingIsFailure: false },
        );
      }
  }

  let run;
  try {
    run = await http.getJson(`${GITHUB_API}/repos/${repo}/actions/runs/${ctx.installerRunId}`);
  } catch (error) {
    section.error("installer-run", error, { runId: ctx.installerRunId });
    return section;
  }
  const runEvidence = {
    id: run.id,
    path: run.path,
    event: run.event,
    head_branch: run.head_branch,
    head_sha: run.head_sha,
    run_attempt: run.run_attempt,
    status: run.status,
    conclusion: run.conclusion,
    created_at: run.created_at,
    html_url: run.html_url,
    selection: "explicit --installer-run-id; the API does not expose the triggering Release run",
  };
  if (ctx.releaseRun?.updated_at) runEvidence.releaseRunUpdatedAt = ctx.releaseRun.updated_at;
  const problems = [];
  if (run.path !== INSTALLER_WORKFLOW_PATH) problems.push(`workflow path is ${run.path}`);
  if (run.event !== "workflow_run") problems.push(`event is ${run.event}, not workflow_run`);
  // A workflow_run installer is created by a Release run's completion, so one
  // created before that run existed cannot have installed this release. The
  // run's first creation is the bound: reruns move updated_at past it.
  const releaseCreated = Date.parse(ctx.releaseRun?.created_at ?? "");
  if (!Number.isNaN(releaseCreated) && !(Date.parse(run.created_at ?? "") >= releaseCreated)) {
    problems.push(`run was created at ${run.created_at}, before the Release run existed`);
  }
  if (run.repository?.full_name && run.repository.full_name.toLowerCase() !== repo.toLowerCase()) {
    problems.push(`run belongs to ${run.repository.full_name}`);
  }
  if (problems.length) section.failed("installer-run", problems.join("; "), runEvidence);
  else if (run.status !== "completed")
    section.incomplete("installer-run", `run is ${run.status}`, runEvidence);
  else if (run.conclusion !== "success")
    section.failed("installer-run", `run concluded ${run.conclusion}`, runEvidence);
  else section.verified("installer-run", runEvidence);

  let grouped;
  try {
    grouped = effectiveJobs(await listAllJobs(http, repo, run.id));
  } catch (error) {
    section.error("installer-job", error, {}, { missingIsFailure: false });
    return section;
  }
  const job = checkRequiredJob(
    section,
    grouped,
    { id: "installer", name: INSTALLER_JOB_NAME, success: INSTALLER_STEPS },
    "job",
  );
  if (!job || job.status !== "completed" || job.conclusion !== "success") return section;
  if (!ctx.commit) {
    section.incomplete(
      "installer-evidence",
      "the tag commit is unresolved, so the installer checkout cannot be matched to it",
      { job: jobSummary(job) },
    );
    return section;
  }
  try {
    const lines = await readJobLog(ctx, repo, job);
    const linuxArchive = `zfb-${version}-x86_64-unknown-linux-gnu.tar.gz`;
    const { found, missing } = logEvidence(lines, {
      checkoutReleaseSource: new RegExp(`fetch .*origin \\+?${ctx.commit}\\b`),
      releaseTagInput: `RELEASE_TAG: ${tag}`,
      installerTag: new RegExp(`tag:\\s+${escapeRegExp(tag)}$`),
      installerSemver: new RegExp(`semver:\\s+${escapeRegExp(version)}$`),
      archiveDownload: `Downloading ${linuxArchive}...`,
      checksumDownload: `Downloading ${linuxArchive}.sha256...`,
      checksumVerified: /(?:^|\s)checksum OK$/,
      installedVersion: `Installed zfb ${version} to`,
      smokeAssertions: "All smoke assertions passed.",
    });
    const scriptLine = lines.find((line) =>
      /raw\.githubusercontent\.com\/[^ ]+\/install\.sh/.test(line),
    );
    const scriptRef = /raw\.githubusercontent\.com\/[^/]+\/[^/]+\/([^/ ]+)\/install\.sh/.exec(
      scriptLine ?? "",
    )?.[1];
    const evidence = {
      found,
      installScriptRef: scriptRef ?? null,
      note:
        scriptRef && !/^[0-9a-f]{40}$/.test(scriptRef)
          ? `install.sh was fetched from the moving ref "${scriptRef}"; this proves exact-version installer behavior, not immutable installer-script bytes`
          : undefined,
    };
    if (missing.length)
      section.failed(
        "installer-evidence",
        `log lacks exact-version evidence: ${missing.join(", ")}`,
        evidence,
      );
    else section.verified("installer-evidence", evidence);
  } catch (error) {
    section.error(
      "installer-evidence",
      error,
      { job: jobSummary(job) },
      { missingIsFailure: false },
    );
  }
  return section;
}

// ── Homebrew formula binding ────────────────────────────────────────────────

// The four delivery rows written by scripts/update-homebrew-formula.sh. Each
// on_<os>/on_<cpu> block must download exactly that target's tar.gz.
export const HOMEBREW_ROWS = Object.freeze([
  { os: "on_macos", cpu: "on_arm", target: "aarch64-apple-darwin" },
  { os: "on_macos", cpu: "on_intel", target: "x86_64-apple-darwin" },
  { os: "on_linux", cpu: "on_arm", target: "aarch64-unknown-linux-gnu" },
  { os: "on_linux", cpu: "on_intel", target: "x86_64-unknown-linux-gnu" },
]);

const FORMULA_BLOCK_START = /^(?:class|module|def|if|unless|case|begin|while|until|for)\b/;
const FORMULA_DO_BLOCK = /\bdo(?:\s*\|[^|]*\|)?$/;
const FORMULA_HEREDOC = /<<[~-]?(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1/;
const FORMULA_STRING_DECL = /^(url|sha256|version)\s+"([^"\\#]*)"$/;
const FORMULA_DELIVERY_KEYWORD = /^(?:url|sha256|mirror|version)\b/;

function stripFormulaComment(line) {
  // Generated code lines never contain `#` inside a string; a `#` that starts
  // the line or follows whitespace begins a comment.
  const index = line.search(/(?:^|\s)#/);
  return (index === -1 ? line : line.slice(0, index)).trim();
}

/**
 * Bounded recognizer for the literal formula format the generator writes. It
 * tracks block nesting by keyword/`do`/`end` lines, skips heredoc bodies, and
 * reads only whole-line `url "…"`, `sha256 "…"` and `version "…"`
 * declarations. Anything else that declares delivery is reported, never
 * guessed at; other content (desc, install, test) is ignored.
 */
export function parseHomebrewFormula(text) {
  const problems = [];
  const stack = [];
  const rows = new Map(HOMEBREW_ROWS.map((row) => [row.target, { urls: [], sha256s: [] }]));
  const versions = [];
  let heredoc = null;
  const lines = String(text).split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const lineNo = index + 1;
    if (heredoc) {
      if (lines[index].trim() === heredoc) heredoc = null;
      continue;
    }
    const code = stripFormulaComment(lines[index]);
    if (!code) continue;
    const heredocStart = FORMULA_HEREDOC.exec(code);
    if (heredocStart) heredoc = heredocStart[2];
    if (code === "end") {
      if (stack.length === 0) problems.push(`line ${lineNo}: unbalanced end`);
      else stack.pop();
      continue;
    }
    const firstWord = /^[A-Za-z_][A-Za-z0-9_]*/.exec(code)?.[0] ?? "";
    if (FORMULA_DO_BLOCK.test(code)) {
      stack.push(firstWord);
      continue;
    }
    if (FORMULA_BLOCK_START.test(code)) {
      stack.push(firstWord === "class" ? "class" : firstWord);
      continue;
    }
    if (!FORMULA_DELIVERY_KEYWORD.test(code)) continue;
    const decl = FORMULA_STRING_DECL.exec(code);
    const where = stack.join(" > ") || "top level";
    if (!decl) {
      problems.push(`line ${lineNo}: unsupported delivery declaration in ${where}: ${code}`);
      continue;
    }
    const [, keyword, value] = decl;
    if (keyword === "version") {
      if (stack.length === 1 && stack[0] === "class") versions.push(value);
      else problems.push(`line ${lineNo}: misplaced version declaration in ${where}`);
      continue;
    }
    const row =
      stack.length === 3 && stack[0] === "class"
        ? HOMEBREW_ROWS.find((r) => r.os === stack[1] && r.cpu === stack[2])
        : undefined;
    if (!row) {
      problems.push(`line ${lineNo}: misplaced ${keyword} declaration in ${where}`);
      continue;
    }
    rows.get(row.target)[keyword === "url" ? "urls" : "sha256s"].push(value);
  }
  if (heredoc) problems.push(`unterminated heredoc ${heredoc}`);
  if (stack.length) problems.push(`unclosed block(s): ${stack.join(" > ")}`);
  if (versions.length !== 1) {
    problems.push(`expected one class-level version declaration, found ${versions.length}`);
  }
  for (const { os, cpu, target } of HOMEBREW_ROWS) {
    const { urls, sha256s } = rows.get(target);
    if (urls.length !== 1) problems.push(`${os}/${cpu}: expected one url, found ${urls.length}`);
    if (sha256s.length !== 1) {
      problems.push(`${os}/${cpu}: expected one sha256, found ${sha256s.length}`);
    }
  }
  return {
    version: versions.length === 1 ? versions[0] : null,
    rows: HOMEBREW_ROWS.map(({ os, cpu, target }) => ({
      os,
      cpu,
      target,
      url: rows.get(target).urls.length === 1 ? rows.get(target).urls[0] : null,
      sha256: rows.get(target).sha256s.length === 1 ? rows.get(target).sha256s[0] : null,
    })),
    problems,
  };
}

/** Per-target identity of the release archives the archives section checked. */
export function archiveIdentityByTarget(archivesSection) {
  const identity = new Map();
  for (const check of archivesSection.checks) {
    if (!check.id.startsWith("archive:")) continue;
    identity.set(check.id.slice("archive:".length), {
      status: check.status,
      name: check.evidence?.archive?.name ?? null,
      url: check.evidence?.archive?.url ?? null,
      sha256: check.evidence?.archive?.computedSha256 ?? null,
    });
  }
  return identity;
}

/**
 * Bind each Homebrew delivery row to its own OS/CPU target: the requested
 * version, the expected release-asset URL, and the digest verified for that
 * same target's archive. Structural or binding mismatches fail; a row whose
 * archive evidence is missing or not verified is incomplete, never verified.
 */
export function checkHomebrewFormula(formula, { version, repo, tag, archives }) {
  const parsed = parseHomebrewFormula(formula);
  const problems = [...parsed.problems];
  if (parsed.version !== null && parsed.version !== version) {
    problems.push(`formula version is ${parsed.version}, expected ${version}`);
  }
  const missingEvidence = [];
  const rows = parsed.rows.map(({ os, cpu, target, url, sha256 }) => {
    const name = `zfb-${version}-${target}.tar.gz`;
    const expectedUrl = `https://github.com/${repo}/releases/download/${tag}/${name}`;
    const archive = archives.get(target);
    const archiveStatus = archive?.status ?? "missing";
    const expectedSha256 = archiveStatus === STATUS.VERIFIED ? archive.sha256 : null;
    const row = {
      os,
      cpu,
      target,
      expected: { url: expectedUrl, sha256: expectedSha256, archiveStatus },
      observed: { url, sha256 },
    };
    if (url !== null && url !== expectedUrl) {
      problems.push(`${os}/${cpu}: url ${url}, expected ${expectedUrl}`);
    }
    if (archiveStatus !== STATUS.VERIFIED) {
      missingEvidence.push(`${os}/${cpu}: ${target} archive evidence is ${archiveStatus}`);
    } else if (sha256 !== null && sha256 !== expectedSha256) {
      problems.push(`${os}/${cpu}: sha256 ${sha256} is not the verified ${target} digest`);
    }
    row.matches = url === expectedUrl && expectedSha256 !== null && sha256 === expectedSha256;
    return row;
  });
  const status = problems.length
    ? STATUS.FAILED
    : missingEvidence.length
      ? STATUS.INCOMPLETE
      : STATUS.VERIFIED;
  return {
    status,
    version: parsed.version,
    rows,
    problems: [...problems, ...missingEvidence],
  };
}

async function observeChannels(ctx) {
  const section = new Section("channels");
  const { http, repo, version, tag, expect } = ctx;
  const record = (id, expected, observedValue, matches, evidence, error) => {
    if (error) {
      section.add(id, expected ? STATUS.INCOMPLETE : STATUS.OBSERVED, evidence, {
        reason: error.message,
        error: errorRecord(error),
      });
    } else if (!expected) section.add(id, STATUS.OBSERVED, evidence);
    else section.expect(id, matches, evidence, `${id} is ${observedValue}, expected ${version}`);
  };
  try {
    const latest = await http.getJson(`${GITHUB_API}/repos/${repo}/releases/latest`);
    record("github-latest", expect.githubLatest, latest.tag_name, latest.tag_name === tag, {
      tag_name: latest.tag_name,
      id: latest.id,
      pointsAtThisRelease: latest.tag_name === tag,
    });
  } catch (error) {
    // No Latest release at all is a definite answer, not missing evidence.
    if (error.kind === "not-found" && expect.githubLatest) {
      section.failed("github-latest", `no release is marked Latest, expected ${tag}`, {
        error: errorRecord(error),
      });
    } else record("github-latest", expect.githubLatest, undefined, false, {}, error);
  }
  const tags = await mapLimit(PUBLISHED_PACKAGES, ctx.concurrency, async (name) => {
    try {
      return {
        name,
        distTags: await http.getJson(`${NPM_REGISTRY}/-/package/${registryPath(name)}/dist-tags`),
      };
    } catch (error) {
      return { name, error };
    }
  });
  const tagEvidence = {
    packages: tags.map((entry) => ({
      name: entry.name,
      distTags: entry.distTags ?? null,
      error: entry.error && errorRecord(entry.error),
    })),
  };
  const wrongTags = tags.filter((entry) => !entry.error && entry.distTags?.latest !== version);
  const failedTag = tags.find((entry) => entry.error);
  if (expect.npmLatest && wrongTags.length) {
    // A package read back with the wrong latest is a failure even when another
    // package's read failed; the failure must not be downgraded to incomplete.
    section.failed(
      "npm-dist-tags",
      `latest is not ${version} for ${wrongTags.map((e) => `${e.name}=${e.distTags?.latest}`).join(", ")}`,
      tagEvidence,
    );
  } else {
    record(
      "npm-dist-tags",
      expect.npmLatest,
      tags.map((entry) => `${entry.name}=${entry.distTags?.latest}`).join(","),
      wrongTags.length === 0,
      tagEvidence,
      failedTag?.error,
    );
  }
  try {
    const formula = await http.getText(HOMEBREW_FORMULA_URL);
    const result = checkHomebrewFormula(formula, {
      version,
      repo,
      tag,
      archives: ctx.archiveIdentity ?? new Map(),
    });
    const evidence = {
      url: HOMEBREW_FORMULA_URL,
      version: result.version,
      rows: result.rows,
      matchesThisReleaseArchives: result.status === STATUS.VERIFIED,
      ...(result.problems.length ? { problems: result.problems } : {}),
    };
    if (!expect.homebrew) section.add("homebrew", STATUS.OBSERVED, evidence);
    else if (result.status === STATUS.VERIFIED) section.verified("homebrew", evidence);
    else if (result.status === STATUS.INCOMPLETE) {
      section.incomplete("homebrew", result.problems.join("; "), evidence);
    } else section.failed("homebrew", result.problems.join("; "), evidence);
  } catch (error) {
    record("homebrew", expect.homebrew, undefined, false, { url: HOMEBREW_FORMULA_URL }, error);
  }
  return section;
}

// ── Orchestration ───────────────────────────────────────────────────────────

/**
 * Run every section and return the versioned report. Network, sigstore, git
 * and clock are injectable so the offline tests can drive every branch.
 */
export async function verifyReleaseDelivery(options, deps = {}) {
  const version = options.version;
  const repo = options.repo ?? DEFAULT_REPO;
  const http =
    deps.http ??
    createHttpClient({
      fetchImpl: deps.fetchImpl,
      token: options.token,
      timeoutMs: options.timeoutMs,
      retries: options.retries,
      sleep: deps.sleep,
    });
  const ownTuf = deps.tufCachePath
    ? undefined
    : mkdtempSync(join(tmpdir(), "zfb-verify-release-tuf-"));
  const ctx = {
    http,
    repo,
    canonicalRepo: repo,
    version,
    tag: `v${version}`,
    releaseRunId: String(options.releaseRunId),
    installerRunId: String(options.installerRunId),
    expectedSource: options.expectedSource,
    expect: {
      githubLatest: Boolean(options.expectGithubLatest),
      npmLatest: Boolean(options.expectNpmLatest),
      homebrew: Boolean(options.expectHomebrew),
    },
    concurrency: options.concurrency ?? 2,
    timeoutMs: options.timeoutMs ?? 60_000,
    verifyBundle: "verifyBundle" in deps ? deps.verifyBundle : createSigstoreBundleVerifier(),
    tufCachePath: deps.tufCachePath ?? ownTuf,
    control: deps.control ?? controlIdentity({ runGit: deps.runGit }),
  };
  try {
    const identity = await verifyIdentity(ctx);
    const workflow = await verifyReleaseWorkflow(ctx);
    const archives = await verifyArchives(ctx);
    ctx.archiveIdentity = archiveIdentityByTarget(archives);
    const npm = await verifyNpm(ctx);
    const wasm = await verifyWasm(ctx);
    const consumption = await verifyConsumption(ctx);
    const channels = await observeChannels(ctx);
    const sections = { identity, workflow, archives, npm, wasm, consumption, channels };
    const sectionJson = Object.fromEntries(
      Object.entries(sections).map(([key, value]) => [key, value.toJSON()]),
    );
    const verdict = combineStatus(Object.values(sectionJson).map((section) => section.status));
    const final = verdict === STATUS.NOT_APPLICABLE ? STATUS.INCOMPLETE : verdict;
    const unverified = [];
    for (const [key, section] of Object.entries(sectionJson)) {
      for (const check of section.checks) {
        if (check.status === STATUS.FAILED || check.status === STATUS.INCOMPLETE) {
          unverified.push({
            section: key,
            id: check.id,
            status: check.status,
            reason: check.reason ?? null,
          });
        }
      }
    }
    return {
      schemaVersion: REPORT_SCHEMA_VERSION,
      tool: "scripts/verify-release-delivery.mjs",
      observedAt: new Date((deps.now ?? Date.now)()).toISOString(),
      control: ctx.control,
      request: {
        repo,
        version,
        tag: ctx.tag,
        releaseRunId: ctx.releaseRunId,
        installerRunId: ctx.installerRunId,
        expectedSource: ctx.expectedSource ?? null,
        expectations: ctx.expect,
      },
      release: {
        tag: ctx.tag,
        commit: ctx.commit ?? null,
        releaseId: ctx.release?.id ?? null,
        deliveryPath: ctx.deliveryPath ?? null,
      },
      verdict: final,
      exitCode: EXIT[final],
      sections: sectionJson,
      unverified,
      limitations: [
        "Foreign-platform binaries are hashed and inspected, never executed.",
        "The installer run is the explicitly selected run; GitHub's API does not name the Release run that triggered a workflow_run.",
        "Mutable channels (GitHub Latest, npm dist-tags, Homebrew) are observations unless an --expect-* flag was given.",
        "Only the normal provenance-enabled release path is supported; dry-run, recovery, mixed-provenance and manual paths are classified, never accepted.",
      ],
      requestCount: http.requests?.length ?? null,
    };
  } finally {
    if (ownTuf) rmSync(ownTuf, { recursive: true, force: true });
  }
}

export function summarize(report) {
  const lines = [
    `Release delivery ${report.request.tag}: ${report.verdict.toUpperCase()} (exit ${report.exitCode})`,
    `  source ${report.release.commit ?? "unresolved"} | path ${report.release.deliveryPath ?? "unknown"} | verifier ${report.control.commit ?? "unknown"}${report.control.dirty ? " (dirty)" : ""}`,
  ];
  for (const [key, section] of Object.entries(report.sections)) {
    const counts = {};
    for (const check of section.checks) counts[check.status] = (counts[check.status] ?? 0) + 1;
    const tally = Object.entries(counts)
      .map(([status, count]) => `${count} ${status}`)
      .join(", ");
    lines.push(`  ${key.padEnd(12)} ${section.status.padEnd(14)} ${tally}`);
    for (const check of section.checks) {
      if (check.status === STATUS.FAILED || check.status === STATUS.INCOMPLETE) {
        lines.push(`    - ${check.status}: ${check.id}: ${check.reason ?? ""}`);
      }
    }
  }
  const channels = report.sections.channels?.checks ?? [];
  const latest = channels.find((check) => check.id === "github-latest")?.evidence?.tag_name;
  const brew = channels.find((check) => check.id === "homebrew")?.evidence?.version;
  lines.push(`  channels observed: GitHub Latest=${latest ?? "?"}, Homebrew=${brew ?? "?"}`);
  return lines.join("\n");
}

const USAGE =
  "usage: verify-release-delivery.mjs --version X.Y.Z --release-run-id ID --installer-run-id ID [--json PATH] [--repo OWNER/NAME] [--expected-source SHA] [--expect-github-latest] [--expect-npm-latest] [--expect-homebrew] [--timeout-ms N] [--retries N] [--concurrency N]";

export function parseArgs(argv) {
  const options = {};
  const flags = {
    "--expect-github-latest": "expectGithubLatest",
    "--expect-npm-latest": "expectNpmLatest",
    "--expect-homebrew": "expectHomebrew",
  };
  const values = {
    "--version": "version",
    "--release-run-id": "releaseRunId",
    "--installer-run-id": "installerRunId",
    "--json": "json",
    "--repo": "repo",
    "--expected-source": "expectedSource",
    "--timeout-ms": "timeoutMs",
    "--retries": "retries",
    "--concurrency": "concurrency",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") return { help: true };
    if (flags[arg]) options[flags[arg]] = true;
    else if (values[arg]) {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) throw new Error(`${arg} needs a value`);
      options[values[arg]] = value;
      index += 1;
    } else throw new Error(`unknown argument ${arg}`);
  }
  for (const required of ["version", "releaseRunId", "installerRunId"]) {
    if (!options[required])
      throw new Error(`missing --${required.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`);
  }
  if (!SEMVER_RE.test(options.version)) throw new Error(`invalid version ${options.version}`);
  for (const key of ["releaseRunId", "installerRunId"]) {
    if (!/^[1-9]\d*$/.test(options[key])) throw new Error(`${key} must be a numeric run ID`);
  }
  if (options.repo && !/^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/.test(options.repo))
    throw new Error("invalid --repo");
  if (options.expectedSource && !/^[0-9a-f]{40}$/.test(options.expectedSource)) {
    throw new Error("--expected-source must be a full 40-hex commit");
  }
  for (const [key, min, max] of [
    ["timeoutMs", 1000, 600_000],
    ["retries", 0, 10],
    ["concurrency", 1, 4],
  ]) {
    if (options[key] === undefined) continue;
    const number = Number(options[key]);
    if (!Number.isInteger(number) || number < min || number > max)
      throw new Error(`--${key} must be ${min}..${max}`);
    options[key] = number;
  }
  return options;
}

async function main(argv) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    console.error(`verify-release-delivery: ${error.message}\n${USAGE}`);
    return EXIT.usage;
  }
  if (options.help) {
    console.log(USAGE);
    return EXIT.verified;
  }
  options.token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN || undefined;
  const report = await verifyReleaseDelivery(options);
  if (options.json) writeFileSync(resolve(options.json), `${JSON.stringify(report, null, 2)}\n`);
  console.log(summarize(report));
  return report.exitCode;
}

function resolvedPath(path) {
  try {
    return realpathSync(path);
  } catch {
    return null;
  }
}
const invokedPath = process.argv[1] && resolvedPath(process.argv[1]);
if (invokedPath && invokedPath === resolvedPath(fileURLToPath(import.meta.url))) {
  main(process.argv.slice(2)).then(
    (code) => {
      process.exitCode = code;
    },
    (error) => {
      console.error(`verify-release-delivery: internal error: ${error?.stack ?? error}`);
      process.exitCode = EXIT.usage;
    },
  );
}
