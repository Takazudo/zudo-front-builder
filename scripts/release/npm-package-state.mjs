#!/usr/bin/env node
// npm-package-state.mjs — read-only per-package registry state for one release
// version, bound to the release SOURCE commit (#4690).
//
//   node scripts/release/npm-package-state.mjs --version V --source-commit SHA
//        [--admission verdict.json] [--package NAME ...] [--tarball-integrity SRI] [--json | --tsv]
//        [--require-all-verified] [--expectations-out FILE] [--registry URL]
//
// States:
//   absent              every bounded re-read answered a definitive 404
//                       (E404/ETARGET); nothing else is ever called absent.
//   present-verified    the version exists AND its identity matches the source
//                       commit (see below).
//   present-unverified  the version exists but nothing ties it to the source
//                       commit, or something contradicts it. Fail closed.
//   uncertain           a network/5xx/auth/parse failure; the original error is
//                       kept in `error`. Fail closed.
//
// Identity, in order:
//   1. SLSA provenance: the attestation bundle's DSSE payload subject digest
//      equals dist.integrity and its resolvedDependencies gitCommit equals SHA.
//      An attestation naming another commit is present-unverified.
//   2. otherwise, with --tarball-integrity, the registry dist.integrity equals
//      the locally packed tarball's (an exact byte match);
//   3. otherwise `gitHead` equals SHA (npm records it for platform carriers).
//   4. otherwise, for a Darwin carrier with --admission, the packed `zfb` and
//      `zfb-css.node` equal the admitted binary/addon digests.
//   With --admission, a Darwin carrier must ALSO match the admitted digests even
//   when (1)-(3) matched: its bytes come only from the owner-Mac admission.
//
// This proves binding, not authenticity: the attestation signature is checked
// by scripts/verify-release-delivery.mjs, not here.
//
// Exit 0 evaluated (and, with --require-all-verified, every package is
// present-verified); 1 --require-all-verified not satisfied; 2 usage error.
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { parseTarGz } from "../admit-darwin-artifacts.mjs";
import { PUBLISHED_PACKAGES } from "../retire-next-dist-tag.mjs";
import { isMain } from "./gh.mjs";

export const DEFAULT_REGISTRY = "https://registry.npmjs.org";
export const SLSA_PROVENANCE_V1 = "https://slsa.dev/provenance/v1";

export const DARWIN_CARRIERS = Object.freeze({
  "@takazudo/zfb-darwin-arm64": "aarch64-apple-darwin",
  "@takazudo/zfb-darwin-x64": "x86_64-apple-darwin",
});

export const STATES = Object.freeze([
  "absent",
  "present-verified",
  "present-unverified",
  "uncertain",
]);

class ReadError extends Error {
  constructor(message, { status = null, cause } = {}) {
    super(message);
    this.status = status;
    this.cause = cause;
  }
}

function registryPath(name) {
  return name.startsWith("@") ? `@${encodeURIComponent(name.slice(1))}` : encodeURIComponent(name);
}

function sleep(ms) {
  return ms > 0 ? new Promise((resolve) => setTimeout(resolve, ms)) : Promise.resolve();
}

/** Default HTTP reader: { status, json(), bytes() } or a thrown ReadError. */
export async function defaultRequest(url, { timeoutMs = 30000 } = {}) {
  let response;
  try {
    response = await fetch(url, { signal: AbortSignal.timeout(timeoutMs) });
  } catch (error) {
    throw new ReadError(`GET ${url} failed: ${error.message}`, { cause: error });
  }
  const body = Buffer.from(await response.arrayBuffer());
  return { status: response.status, body };
}

/**
 * Bounded read of <name>@<version>. Returns { record } or { absent: true }.
 * A non-404 failure throws ReadError (→ uncertain) and is never retried into
 * "absent".
 */
export async function readVersion(name, version, { registry, request, absentReads, delayMs }) {
  const url = `${registry}/${registryPath(name)}/${encodeURIComponent(version)}`;
  let lastNotFound = "";
  for (let attempt = 1; attempt <= absentReads; attempt++) {
    const response = await request(url);
    if (response.status === 200) {
      try {
        return { record: JSON.parse(response.body.toString("utf8")) };
      } catch (error) {
        throw new ReadError(`GET ${url}: malformed JSON: ${error.message}`, { status: 200 });
      }
    }
    if (response.status !== 404) {
      throw new ReadError(
        `GET ${url} answered HTTP ${response.status}: ${response.body.toString("utf8").slice(0, 300)}`,
        { status: response.status },
      );
    }
    lastNotFound = response.body.toString("utf8").slice(0, 200);
    if (attempt < absentReads) await sleep(delayMs);
  }
  return { absent: true, detail: `E404 on ${absentReads} read(s): ${lastNotFound}` };
}

function integrityHex(integrity) {
  const match = /^sha512-(.+)$/.exec(integrity ?? "");
  return match ? Buffer.from(match[1], "base64").toString("hex") : null;
}

/** Pure: extract { subjectSha512, gitCommit } from an attestations body. */
export function provenanceIdentity(body) {
  const entries = (body?.attestations ?? []).filter(
    (entry) => entry?.predicateType === SLSA_PROVENANCE_V1,
  );
  if (entries.length !== 1) {
    throw new Error(`expected one SLSA provenance attestation, found ${entries.length}`);
  }
  const envelope = entries[0].bundle?.dsseEnvelope;
  if (envelope?.payloadType !== "application/vnd.in-toto+json" || !envelope.payload) {
    throw new Error("provenance bundle has no in-toto DSSE payload");
  }
  const statement = JSON.parse(Buffer.from(envelope.payload, "base64").toString("utf8"));
  if (statement.predicateType !== SLSA_PROVENANCE_V1) {
    throw new Error(`DSSE statement predicateType is ${statement.predicateType}`);
  }
  const subjects = statement.subject ?? [];
  if (subjects.length !== 1) throw new Error(`statement has ${subjects.length} subjects`);
  const commits = (statement.predicate?.buildDefinition?.resolvedDependencies ?? [])
    .map((dependency) => dependency?.digest?.gitCommit)
    .filter(Boolean);
  if (commits.length !== 1) throw new Error(`statement names ${commits.length} git commits`);
  return {
    subjectName: subjects[0].name,
    subjectSha512: subjects[0].digest?.sha512 ?? null,
    gitCommit: commits[0],
  };
}

/** Pure: compare a packed Darwin carrier to the admission verdict. */
export function darwinDigestProblems(files, verdict, triple) {
  const target = verdict?.targets?.[triple];
  if (!verdict?.admitted || !target?.binary?.sha256 || !target?.addon?.sha256) {
    return [`admission verdict has no admitted ${triple} binary/addon digests`];
  }
  const problems = [];
  for (const [path, expected] of [
    ["package/zfb", target.binary.sha256],
    ["package/zfb-css.node", target.addon.sha256],
  ]) {
    const entry = files.find((file) => file.name === path);
    if (!entry) problems.push(`${path} is missing from the packed carrier`);
    else {
      const actual = createHash("sha256").update(entry.data).digest("hex");
      if (actual !== expected) problems.push(`${path} sha256 ${actual} != admitted ${expected}`);
    }
  }
  return problems;
}

export async function packageState(name, ctx) {
  const { version, sourceCommit, verdict, registry, request } = ctx;
  let read;
  try {
    read = await readVersion(name, version, ctx);
  } catch (error) {
    return { name, state: "uncertain", error: error.message };
  }
  if (read.absent) return { name, state: "absent", detail: read.detail };
  const record = read.record;
  const base = {
    name,
    gitHead: record.gitHead ?? null,
    integrity: record.dist?.integrity ?? null,
    attested: Boolean(record.dist?.attestations?.url),
  };
  if (record.name !== name || record.version !== version) {
    return { ...base, state: "present-unverified", detail: "registry name/version differ" };
  }

  let method = null;
  const integrityMatch =
    Boolean(ctx.tarballIntegrity) && record.dist?.integrity === ctx.tarballIntegrity;
  base.observedProvenance = "none";
  const attestations = record.dist?.attestations;
  if (attestations?.url && attestations.provenance?.predicateType === SLSA_PROVENANCE_V1) {
    let body;
    try {
      const response = await request(attestations.url);
      if (response.status !== 200) {
        throw new ReadError(`GET ${attestations.url} answered HTTP ${response.status}`);
      }
      body = JSON.parse(response.body.toString("utf8"));
    } catch (error) {
      return { ...base, state: "uncertain", error: error.message };
    }
    let identity;
    try {
      identity = provenanceIdentity(body);
    } catch (error) {
      return { ...base, state: "present-unverified", detail: `attestation: ${error.message}` };
    }
    base.provenanceCommit = identity.gitCommit;
    if (identity.subjectSha512 !== integrityHex(record.dist.integrity)) {
      return {
        ...base,
        state: "present-unverified",
        detail: "attestation subject digest differs from dist.integrity",
      };
    }
    if (identity.gitCommit !== sourceCommit) {
      return {
        ...base,
        state: "present-unverified",
        detail: `attested to commit ${identity.gitCommit}, not source ${sourceCommit}`,
      };
    }
    method = "attestation";
    base.observedProvenance = "attested";
    if (integrityMatch) method = "attestation+integrity";
  } else if (integrityMatch) {
    // The registry holds exactly the tarball the pack job built from the
    // source commit: the strongest binding available.
    method = "integrity";
  } else if (record.gitHead) {
    if (record.gitHead !== sourceCommit) {
      return {
        ...base,
        state: "present-unverified",
        detail: `gitHead ${record.gitHead} is not source ${sourceCommit}`,
      };
    }
    method = "gitHead";
  }

  const triple = DARWIN_CARRIERS[name];
  if (triple && verdict) {
    let files;
    try {
      const response = await request(record.dist.tarball);
      if (response.status !== 200) {
        throw new ReadError(`GET ${record.dist.tarball} answered HTTP ${response.status}`);
      }
      const sha512 = createHash("sha512").update(response.body).digest("hex");
      if (sha512 !== integrityHex(record.dist.integrity)) {
        return {
          ...base,
          state: "present-unverified",
          detail: "tarball bytes differ from dist.integrity",
        };
      }
      files = parseTarGz(response.body);
    } catch (error) {
      return { ...base, state: "uncertain", error: error.message };
    }
    const problems = darwinDigestProblems(files, verdict, triple);
    if (problems.length) {
      return { ...base, state: "present-unverified", detail: problems.join("; ") };
    }
    method = method ? `${method}+darwin-digest` : "darwin-digest";
  }

  if (!method) {
    return {
      ...base,
      state: "present-unverified",
      detail: "no attestation, gitHead or admitted digest ties it to the source commit",
    };
  }
  return { ...base, state: "present-verified", method };
}

/**
 * Expected provenance per already-published package: attested exactly when
 * its attestation names the source commit, none otherwise; Darwin carriers are
 * always none (owner-Mac bytes are never attested).
 */
export function expectationShape(results) {
  return results.map((result) => ({
    name: result.name,
    // Per package, from what the registry observably carries for the source
    // commit: a partial normal publish resumed by recovery is mixed (#4690).
    expected:
      !DARWIN_CARRIERS[result.name] && result.observedProvenance === "attested"
        ? "attested"
        : "none",
  }));
}

export async function collectStates({
  version,
  sourceCommit,
  verdict = null,
  packages = PUBLISHED_PACKAGES,
  registry = DEFAULT_REGISTRY,
  request = defaultRequest,
  absentReads = 3,
  delayMs = 2000,
  tarballIntegrity = null,
}) {
  const ctx = {
    version,
    sourceCommit,
    verdict,
    registry,
    request,
    absentReads,
    delayMs,
    tarballIntegrity,
  };
  const results = [];
  for (const name of packages) results.push(await packageState(name, ctx));
  const byState = Object.fromEntries(STATES.map((state) => [state, []]));
  for (const result of results) byState[result.state].push(result.name);
  return {
    schema: "zfb-npm-package-state/v1",
    version,
    sourceCommit,
    admission: verdict ? { admitted: Boolean(verdict.admitted) } : null,
    packages: results,
    summary: byState,
    allVerified: byState["present-verified"].length === results.length,
  };
}

const SHA_RE = /^[0-9a-f]{40}$/;

export function parseArgs(argv) {
  const opts = { packages: [], format: "text", requireAllVerified: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--json") opts.format = "json";
    else if (arg === "--tsv") opts.format = "tsv";
    else if (arg === "--require-all-verified") opts.requireAllVerified = true;
    else if (
      [
        "--version",
        "--source-commit",
        "--admission",
        "--package",
        "--registry",
        "--tarball-integrity",
      ].includes(arg) ||
      arg === "--expectations-out"
    ) {
      const value = argv[++i];
      if (!value) return null;
      if (arg === "--package") opts.packages.push(value);
      else opts[arg.slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value;
    } else return null;
  }
  if (!opts.version || !opts.sourceCommit || !SHA_RE.test(opts.sourceCommit)) return null;
  for (const name of opts.packages) if (!PUBLISHED_PACKAGES.includes(name)) return null;
  // One local tarball describes one package.
  if (opts.tarballIntegrity && opts.packages.length !== 1) return null;
  return opts;
}

export async function main(argv, { env = process.env, out = process.stdout, request } = {}) {
  const opts = parseArgs(argv);
  if (!opts) {
    process.stderr.write(
      "usage: npm-package-state.mjs --version V --source-commit <40-hex> [--admission FILE] [--package NAME ...] [--tarball-integrity sha512-… (one --package)] [--json|--tsv] [--require-all-verified] [--expectations-out FILE] [--registry URL]\n",
    );
    return 2;
  }
  let verdict = null;
  if (opts.admission) {
    try {
      verdict = JSON.parse(readFileSync(opts.admission, "utf8"));
    } catch (error) {
      process.stderr.write(`npm-package-state: cannot read --admission: ${error.message}\n`);
      return 2;
    }
  }
  const report = await collectStates({
    version: opts.version,
    sourceCommit: opts.sourceCommit,
    verdict,
    packages: opts.packages.length ? opts.packages : PUBLISHED_PACKAGES,
    registry: opts.registry ?? env.ZFB_NPM_REGISTRY ?? DEFAULT_REGISTRY,
    request: request ?? defaultRequest,
    absentReads: Number(env.ZFB_NPM_STATE_ABSENT_READS ?? 3),
    delayMs: Number(env.ZFB_NPM_STATE_DELAY_MS ?? 2000),
    tarballIntegrity: opts.tarballIntegrity ?? null,
  });
  if (opts.expectationsOut) {
    const lines = expectationShape(report.packages).map(
      ({ name, expected }) => `${name}\t${opts.version}\t${expected}\talready-present\n`,
    );
    writeFileSync(opts.expectationsOut, lines.join(""));
  }
  if (opts.format === "json") out.write(`${JSON.stringify(report, null, 2)}\n`);
  else if (opts.format === "tsv") {
    for (const result of report.packages) {
      const detail = (result.error ?? result.detail ?? result.method ?? "").replace(/\s+/g, " ");
      out.write(
        `${result.name}\t${result.state}\t${detail}\t${result.observedProvenance ?? "none"}\n`,
      );
    }
  } else {
    for (const result of report.packages) {
      const detail = result.error ?? result.detail ?? result.method ?? "";
      out.write(`  ${result.state.padEnd(19)} ${result.name}@${opts.version}  ${detail}\n`);
    }
  }
  if (opts.requireAllVerified && !report.allVerified) return 1;
  return 0;
}

if (isMain(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
