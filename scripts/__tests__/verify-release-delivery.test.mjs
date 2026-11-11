// Offline tests for scripts/verify-release-delivery.mjs.
//
// Every test drives the verifier against an in-memory fake of the GitHub API,
// the npm registry, the Homebrew tap and sigstore. Nothing here touches the
// network, a subprocess or a real notification sink; the fake fetch also
// records every request so the read-only contract is asserted directly.

import { createHash, generateKeyPairSync, sign as cryptoSign } from "node:crypto";
import { gunzipSync, gzipSync } from "node:zlib";

import { describe, expect, it } from "vite-plus/test";

import { PUBLISHED_PACKAGES } from "../retire-next-dist-tag.mjs";
import {
  CI_BUILD_TARGETS,
  EXIT,
  FULCIO_OIDS,
  OWNER_MAC_CARRIERS,
  HOMEBREW_FORMULA_URL,
  PLATFORM_CARRIERS,
  PLATFORM_TARGETS,
  STATUS,
  combineStatus,
  createHttpClient,
  decodeExtensionValue,
  effectiveJobs,
  escapeRegExp,
  npmPurl,
  parseArgs,
  parseTar,
  summarize,
  verifyReleaseDelivery,
} from "../verify-release-delivery.mjs";
import { MD_WASM_PATHS } from "../verify-zfb-md-wasm-published-digests.mjs";

const REPO = "Takazudo/zudo-front-builder";
const API = `https://api.github.com/repos/${REPO}`;
const REG = "https://registry.npmjs.org";
const VERSION = "5.2.2";
const TAG = `v${VERSION}`;
const COMMIT = "282551e6e59e535698bb2bcbbf15b967d672743d";
const RELEASE_RUN = 42176242188;
const INSTALLER_RUN = 42181081547;
const IDENTITY = `https://github.com/${REPO}/.github/workflows/release.yml@refs/tags/${TAG}`;

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const sha512hex = (bytes) => createHash("sha512").update(bytes).digest("hex");

// ── tiny tar.gz writer for package fixtures ─────────────────────────────────

function tarHeader(name, size, mode) {
  const header = Buffer.alloc(512);
  header.write(name, 0, 100, "utf8");
  header.write(`${mode.toString(8).padStart(7, "0")}\0`, 100, 8, "ascii");
  header.write("0000000\0", 108, 8, "ascii");
  header.write("0000000\0", 116, 8, "ascii");
  header.write(`${size.toString(8).padStart(11, "0")}\0`, 124, 12, "ascii");
  header.write("00000000000\0", 136, 12, "ascii");
  header.write("        ", 148, 8, "ascii");
  header.write("0", 156, 1, "ascii");
  header.write("ustar\0", 257, 6, "ascii");
  header.write("00", 263, 2, "ascii");
  let sum = 0;
  for (const byte of header) sum += byte;
  header.write(`${sum.toString(8).padStart(6, "0")}\0 `, 148, 8, "ascii");
  return header;
}

function tgz(files) {
  const parts = [];
  for (const [name, { data, mode = 0o644 }] of Object.entries(files)) {
    const bytes = Buffer.from(data);
    parts.push(tarHeader(`package/${name}`, bytes.length, mode), bytes);
    parts.push(Buffer.alloc((512 - (bytes.length % 512)) % 512));
  }
  parts.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(parts));
}

function rawTgz(files) {
  const parts = [];
  for (const [name, { data, mode = 0o644 }] of Object.entries(files)) {
    const bytes = Buffer.from(data);
    parts.push(tarHeader(name, bytes.length, mode), bytes);
    parts.push(Buffer.alloc((512 - (bytes.length % 512)) % 512));
  }
  parts.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(parts));
}

function integrityOf(bytes) {
  return `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
}

// ── fake world ──────────────────────────────────────────────────────────────

function step(name, conclusion = "success") {
  return { name, status: "completed", conclusion };
}

function releaseJobs() {
  let id = 1000;
  const job = (name, steps, extra = {}) => ({
    id: id++,
    name,
    run_attempt: 1,
    status: "completed",
    conclusion: "success",
    html_url: `https://github.com/${REPO}/actions/runs/${RELEASE_RUN}/job/${id}`,
    steps,
    ...extra,
  });
  return [
    job("Resolve and verify release source", [
      step("Verify immutable source and published Release"),
    ]),
    job("Verify source commit required CI", [step("Verify required CI on the source commit")]),
    job("Provenance downgrade preflight", [step("Provenance downgrade preflight")]),
    job("Admit owner-Mac Darwin artifacts", [step("Download and admit the ten Darwin inputs")]),
    ...CI_BUILD_TARGETS.map(({ target, ext }) =>
      job(`Build ${target}`, [
        step("Create archive (Unix tar.gz)", ext === "zip" ? "skipped" : "success"),
        step("Create archive (Windows zip)", ext === "zip" ? "success" : "skipped"),
      ]),
    ),
    job("Build @takazudo/zfb-md-wasm npm package", [
      step("Re-assert final @takazudo/zfb-md-wasm dist"),
    ]),
    job("Upload GitHub Release assets", [
      step("DRY RUN — skip GH Release upload", "skipped"),
      step("Guard — verified release tag required for real release upload"),
      step("Upload archives and checksums to GitHub Release"),
      step("Verify owner-uploaded Darwin assets are unchanged since admission"),
    ]),
    job("Pack npm tarballs", [
      step("Place admitted Darwin binaries in platform packages"),
      step("Pack npm tarballs"),
    ]),
    job("publish", [
      step("Verify publish context (verified release tag required)"),
      step("Publish to npm"),
      step("Publish to npm (recovery — no provenance)", "skipped"),
    ]),
    job("Upload MD/WASM digest asset", [
      step("Verify published @takazudo/zfb-md-wasm and upload digest asset"),
    ]),
    job("smoke-clean-room / Clean-room smoke (ubuntu-latest, dist_tag=latest)", [
      step("Run clean-room smoke (scaffold + build + assert)"),
    ]),
    job("notify", []),
  ];
}

function installerJobs() {
  return [
    {
      id: 5000,
      name: "Smoke released (install.sh)",
      run_attempt: 1,
      status: "completed",
      conclusion: "success",
      steps: [
        step("Build smoke image (released mode)"),
        step("Run smoke container (released mode)"),
      ],
    },
    {
      id: 5001,
      name: "Smoke amd64 (local mode)",
      run_attempt: 1,
      status: "completed",
      conclusion: "skipped",
      steps: [],
    },
  ];
}

const CLEAN_ROOM_LOG = [
  `2026-11-06T07:25:43.4958860Z Uses: ${REPO}/.github/workflows/reusable-smoke-clean-room.yml@refs/tags/${TAG} (${COMMIT})`,
  `2026-11-06T07:25:43.4962134Z   expected_version: ${VERSION}`,
  `2026-11-06T07:25:57.3184315Z Registry resolved create-zfb@latest -> ${VERSION}`,
  `2026-11-06T07:28:03.6753993Z + @takazudo/zfb ${VERSION}`,
  `2026-11-06T07:28:03.6754479Z + @takazudo/zfb-runtime ${VERSION}`,
  "2026-11-06T07:28:04.8721565Z [PASS] dist/index.html exists",
].join("\n");

const INSTALLER_LOG = [
  `﻿2026-11-06T07:28:22.2335882Z [command]/usr/bin/git -c protocol.version=2 fetch --no-tags --prune --no-recurse-submodules --depth=1 origin ${COMMIT}`,
  `2026-11-06T07:28:24.6062401Z   RELEASE_TAG: ${TAG}`,
  `2026-11-06T07:28:32.0583830Z #9 [4/7] RUN curl -o "$s" https://raw.githubusercontent.com/${REPO}/main/install.sh; ZFB_VERSION="${TAG}" sh "$s"`,
  `2026-11-06T07:28:32.3128938Z #9 0.255   tag:    ${TAG}`,
  `2026-11-06T07:28:32.3129156Z #9 0.255   semver: ${VERSION}`,
  `2026-11-06T07:28:32.3129559Z #9 0.255 Downloading zfb-${VERSION}-x86_64-unknown-linux-gnu.tar.gz...`,
  `2026-11-06T07:28:33.0768123Z #9 1.052 Downloading zfb-${VERSION}-x86_64-unknown-linux-gnu.tar.gz.sha256...`,
  "2026-11-06T07:28:33.4032657Z #9 1.496   checksum OK",
  `2026-11-06T07:28:34.0907977Z #9 2.183 Installed zfb ${VERSION} to /root/.local/bin/zfb`,
  "2026-11-06T07:28:43.0272196Z ==> All smoke assertions passed.",
].join("\n");

function buildPackages() {
  const wasmFiles = {};
  const artifacts = MD_WASM_PATHS.map(([entry, path], index) => {
    const data = Buffer.from(`wasm-${index}-bytes`);
    wasmFiles[path] = { data };
    return { entry, path, bytes: data.length, sha256: sha256(data) };
  });
  const manifestBytes = Buffer.from(
    `${JSON.stringify({ schemaVersion: 1, name: "@takazudo/zfb-md-wasm", version: VERSION, artifacts }, null, 2)}\n`,
  );
  const packages = {};
  for (const name of PUBLISHED_PACKAGES) {
    const carrier = PLATFORM_CARRIERS[name];
    const manifest = { name, version: VERSION };
    const files = {};
    if (carrier) {
      Object.assign(manifest, { os: [carrier.os], cpu: [carrier.cpu] });
      files[carrier.binary] = { data: `binary for ${name}`, mode: 0o755 };
      files["zfb-css.node"] = { data: "addon" };
    }
    if (name === "@takazudo/zfb-md-wasm") {
      Object.assign(files, wasmFiles);
      files["dist/shipped-artifacts.json"] = { data: manifestBytes };
    }
    files["package.json"] = { data: JSON.stringify(manifest) };
    // #4690: owner-Mac Darwin carriers are published with --provenance=false.
    packages[name] = OWNER_MAC_CARRIERS[name] ? { files, noAttestation: true } : { files };
  }
  return { packages, manifestBytes };
}

/** Build a complete, valid v5.2.2 world; `tweak(world)` mutates it per test. */
function makeWorld(tweak = () => {}) {
  const { packages, manifestBytes } = buildPackages();
  const archives = {};
  const ownerMac = Object.fromEntries(
    Object.entries(OWNER_MAC_CARRIERS).map(([pkg, target]) => [target, pkg]),
  );
  for (const { target, ext } of PLATFORM_TARGETS) {
    const name = `zfb-${VERSION}-${target}.${ext}`;
    let bytes;
    if (ownerMac[target]) {
      // Owner-Mac archive: one root `zfb` equal to the published carrier's.
      bytes = rawTgz({ zfb: { data: `binary for ${ownerMac[target]}`, mode: 0o755 } });
      const addon = `zfb-css-${VERSION}-${target}.node`;
      archives[addon] = Buffer.from("addon");
      archives[`${addon}.sha256`] = Buffer.from(`${sha256(archives[addon])}  ${addon}\n`);
    } else {
      bytes =
        ext === "zip"
          ? Buffer.concat([Buffer.from([0x50, 0x4b, 0x03, 0x04]), Buffer.from(target)])
          : gzipSync(Buffer.from(target));
    }
    archives[name] = bytes;
    archives[`${name}.sha256`] = Buffer.from(
      `${sha256(bytes)}  ${name}${ext === "zip" ? "" : "\n"}`,
    );
  }
  archives[`zfb-md-wasm-${VERSION}-shipped-artifacts.json`] = manifestBytes;
  const world = {
    tagRef: { ref: `refs/tags/${TAG}`, object: { sha: COMMIT, type: "commit" } },
    tagObjects: {},
    release: {
      id: 436184322,
      tag_name: TAG,
      draft: false,
      prerelease: false,
      published_at: "2026-11-06T06:43:23Z",
      target_commitish: COMMIT,
    },
    latest: { tag_name: TAG, id: 436184322 },
    sourceVersion: VERSION,
    releaseRun: {
      id: RELEASE_RUN,
      path: ".github/workflows/release.yml",
      event: "release",
      head_branch: TAG,
      head_sha: COMMIT,
      run_attempt: 1,
      status: "completed",
      conclusion: "success",
      repository: { full_name: REPO, id: 1001, owner: { id: 2002 } },
      created_at: "2026-11-06T06:40:00Z",
      updated_at: "2026-11-06T07:28:10Z",
    },
    releaseJobs: releaseJobs(),
    attemptRuns: {},
    installerRun: {
      id: INSTALLER_RUN,
      path: ".github/workflows/node-free-smoke.yml",
      event: "workflow_run",
      head_branch: "main",
      head_sha: "1111111111111111111111111111111111111111",
      run_attempt: 1,
      status: "completed",
      conclusion: "success",
      repository: { full_name: REPO },
      created_at: "2026-11-06T07:28:20Z",
    },
    installerJobs: installerJobs(),
    logs: { [jobIdOf(releaseJobs(), "smoke-clean-room")]: CLEAN_ROOM_LOG, 5000: INSTALLER_LOG },
    archives,
    packages,
    attestationAttempt: () => 1,
    statementTweak: () => {},
    signerTweak: () => {},
    distTags: () => ({ latest: VERSION }),
    formula: null,
    overrides: new Map(),
    jobsPageSize: 100,
  };
  tweak(world);
  for (const [name, pkg] of Object.entries(world.packages)) {
    pkg.bytes ??= tgz(pkg.files);
    pkg.integrity ??= integrityOf(pkg.bytes);
  }
  world.formula ??= generatorFormula(world);
  return world;
}

const RELEASE_DOWNLOAD = `https://github.com/${REPO}/releases/download/${TAG}`;

function archiveSha(world, target) {
  const ext = target.includes("windows") ? "zip" : "tar.gz";
  return sha256(world.archives[`zfb-${VERSION}-${target}.${ext}`] ?? "");
}

/**
 * The exact text scripts/update-homebrew-formula.sh writes. `rows` overrides
 * the url/sha256 pair of a branch, keyed "macos/arm", "macos/intel",
 * "linux/arm" or "linux/intel".
 */
function generatorFormula(world, { version = VERSION, rows = {} } = {}) {
  const row = (key, target) => ({
    url: `${RELEASE_DOWNLOAD}/zfb-${VERSION}-${target}.tar.gz`,
    sha: archiveSha(world, target),
    ...rows[key],
  });
  const dm = row("macos/arm", "aarch64-apple-darwin");
  const di = row("macos/intel", "x86_64-apple-darwin");
  const lm = row("linux/arm", "aarch64-unknown-linux-gnu");
  const li = row("linux/intel", "x86_64-unknown-linux-gnu");
  return `class Zfb < Formula
  desc "Content-site engine on the web's Request/Response model, in one Rust binary"
  homepage "https://takazudomodular.com/pj/zudo-front-builder/"
  # Source: GitHub Releases from Takazudo/zudo-front-builder (not the npm registry).
  # Archive contract: zfb-{semver}-{target-triple}.tar.gz, one file at archive root.
  license "MIT"
  version "${version}"

  on_macos do
    on_arm do
      url "${dm.url}"
      sha256 "${dm.sha}"
    end

    on_intel do
      url "${di.url}"
      sha256 "${di.sha}"
    end
  end

  on_linux do
    on_arm do
      url "${lm.url}"
      sha256 "${lm.sha}"
    end

    on_intel do
      url "${li.url}"
      sha256 "${li.sha}"
    end
  end

  def install
    bin.install "zfb"
  end

  test do
    system bin/"zfb", "--version"
  end
end
`;
}

function jobIdOf(jobs, prefix) {
  return jobs.find((job) => job.name.startsWith(prefix)).id;
}

function json(body, status = 200, headers = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });
}

function assetList(world) {
  let id = 700;
  return Object.keys(world.archives).map((name) => ({
    id: id++,
    name,
    size: world.archives[name].length,
    state: "uploaded",
    digest: `sha256:${sha256(world.archives[name])}`,
    browser_download_url: `https://github.com/${REPO}/releases/download/${TAG}/${name}`,
  }));
}

function makeFetch(world) {
  const calls = [];
  const fetchImpl = async (url, init = {}) => {
    calls.push({ url, method: init.method ?? "GET", body: init.body });
    if (world.overrides.has(url)) return world.overrides.get(url)(url);
    const u = new URL(url);
    const path = `${u.origin}${u.pathname}`;
    if (path === `${API}/git/ref/tags/${TAG}`) return json(world.tagRef);
    const tagObject = /\/git\/tags\/([0-9a-f]+)$/.exec(path);
    if (tagObject)
      return json(world.tagObjects[tagObject[1]] ?? {}, world.tagObjects[tagObject[1]] ? 200 : 404);
    if (path === `${API}/releases/tags/${TAG}`) {
      return world.release
        ? json({ ...world.release, assets: assetList(world) })
        : json({ message: "Not Found" }, 404);
    }
    if (path === `${API}/releases/latest`) return json(world.latest);
    if (path === `${API}/contents/packages/zfb/package.json`) {
      return json({
        content: Buffer.from(JSON.stringify({ version: world.sourceVersion })).toString("base64"),
      });
    }
    const attempt = new RegExp(`/actions/runs/${RELEASE_RUN}/attempts/(\\d+)$`).exec(path);
    if (attempt)
      return json(world.attemptRuns[attempt[1]] ?? { status: "completed", conclusion: "failure" });
    const run = /\/actions\/runs\/(\d+)$/.exec(path);
    if (run) {
      if (run[1] === String(RELEASE_RUN)) return json(world.releaseRun);
      if (run[1] === String(INSTALLER_RUN)) return json(world.installerRun);
      return json({ message: "Not Found" }, 404);
    }
    const jobs = /\/actions\/runs\/(\d+)\/jobs$/.exec(path);
    if (jobs) {
      const all = jobs[1] === String(RELEASE_RUN) ? world.releaseJobs : world.installerJobs;
      const page = Number(u.searchParams.get("page") ?? 1);
      const size = world.jobsPageSize;
      return json({
        total_count: world.jobsTotalOverride ?? all.length,
        jobs: all.slice((page - 1) * size, page * size),
      });
    }
    const log = /\/actions\/jobs\/(\d+)\/logs$/.exec(path);
    if (log) {
      const text = world.logs[log[1]];
      return text === undefined ? new Response("gone", { status: 410 }) : new Response(text);
    }
    const download = new RegExp(`/releases/download/${TAG}/(.+)$`).exec(path);
    if (download) {
      const bytes = world.archives[decodeURIComponent(download[1])];
      return bytes ? new Response(bytes) : new Response("missing", { status: 404 });
    }
    if (url === HOMEBREW_FORMULA_URL) return new Response(world.formula);
    if (url === `${REG}/-/npm/v1/keys`) return json({ keys: world.registryKeys ?? [] });
    const distTags = /^https:\/\/registry\.npmjs\.org\/-\/package\/(.+)\/dist-tags$/.exec(url);
    if (distTags) return json(world.distTags(decodeURIComponent(distTags[1])));
    const attestation = /\/-\/npm\/v1\/attestations\/(.+)@[^@]+$/.exec(url);
    if (attestation) {
      const name = decodeURIComponent(attestation[1]);
      return json({
        attestations: [
          { predicateType: "https://slsa.dev/provenance/v1", bundle: bundleFor(world, name) },
        ],
      });
    }
    const tarball = /^https:\/\/registry\.npmjs\.org\/.+\/-\/(.+)\.tgz$/.exec(url);
    if (tarball) {
      const pkg = Object.values(world.packages).find((candidate) => candidate.tarballUrl === url);
      return pkg
        ? new Response(pkg.servedBytes ?? pkg.bytes)
        : new Response("nope", { status: 404 });
    }
    const meta = /^https:\/\/registry\.npmjs\.org\/(.+)\/([^/]+)$/.exec(url);
    if (meta) {
      const name = decodeURIComponent(meta[1]);
      const pkg = world.packages[name];
      if (!pkg || pkg.unpublished) return json({ error: "not found" }, 404);
      pkg.tarballUrl = `${REG}/${name}/-/${name.split("/").at(-1)}-${VERSION}.tgz`;
      return json({
        name,
        version: meta[2],
        dist: {
          tarball: pkg.tarballUrl,
          integrity: pkg.integrity,
          signatures: pkg.signatures ?? [],
          ...(pkg.noAttestation
            ? {}
            : {
                attestations: {
                  url: `${REG}/-/npm/v1/attestations/${encodeURIComponent(name)}@${VERSION}`,
                  provenance: { predicateType: "https://slsa.dev/provenance/v1" },
                },
              }),
        },
      });
    }
    return new Response(`unexpected ${url}`, { status: 599 });
  };
  return { fetchImpl, calls };
}

function bundleFor(world, name) {
  const pkg = world.packages[name];
  const statement = {
    _type: "https://in-toto.io/Statement/v1",
    subject: [{ name: npmPurl(name, VERSION), digest: { sha512: sha512hex(pkg.bytes) } }],
    predicateType: "https://slsa.dev/provenance/v1",
    predicate: {
      buildDefinition: {
        buildType: "https://slsa-framework.github.io/github-actions-buildtypes/workflow/v1",
        externalParameters: {
          workflow: {
            ref: `refs/tags/${TAG}`,
            repository: `https://github.com/${REPO}`,
            path: ".github/workflows/release.yml",
          },
        },
        internalParameters: { github: { event_name: "release" } },
        resolvedDependencies: [
          { uri: `git+https://github.com/${REPO}@refs/tags/${TAG}`, digest: { gitCommit: COMMIT } },
        ],
      },
      runDetails: {
        metadata: {
          invocationId: `https://github.com/${REPO}/actions/runs/${RELEASE_RUN}/attempts/${world.attestationAttempt(name)}`,
        },
      },
    },
  };
  world.statementTweak(name, statement);
  return {
    mediaType: "application/vnd.dev.sigstore.bundle.v0.3+json",
    dsseEnvelope: {
      payloadType: "application/vnd.in-toto+json",
      payload: Buffer.from(JSON.stringify(statement)).toString("base64"),
      signatures: [],
    },
    zfbTestName: name,
    zfbInvocation: statement.predicate.runDetails.metadata.invocationId,
  };
}

function fakeVerifier(world) {
  const seen = [];
  const verifyBundle = async (bundle, policy) => {
    seen.push({ name: bundle.zfbTestName, policy });
    const signer = {
      subjectAlternativeName: IDENTITY,
      issuer: "https://token.actions.githubusercontent.com",
      oids: {
        [FULCIO_OIDS.issuer]: "https://token.actions.githubusercontent.com",
        [FULCIO_OIDS.buildSignerUri]: IDENTITY,
        [FULCIO_OIDS.buildSignerDigest]: COMMIT,
        [FULCIO_OIDS.runnerEnvironment]: "github-hosted",
        [FULCIO_OIDS.sourceRepositoryUri]: `https://github.com/${REPO}`,
        [FULCIO_OIDS.sourceRepositoryDigest]: COMMIT,
        [FULCIO_OIDS.sourceRepositoryRef]: `refs/tags/${TAG}`,
        [FULCIO_OIDS.sourceRepositoryIdentifier]: "1001",
        [FULCIO_OIDS.sourceRepositoryOwnerIdentifier]: "2002",
        [FULCIO_OIDS.buildTrigger]: "release",
        [FULCIO_OIDS.runInvocationUri]: bundle.zfbInvocation,
      },
    };
    const outcome = world.signerTweak(bundle.zfbTestName, signer);
    if (outcome instanceof Error) throw outcome;
    if (!new RegExp(policy.identityRegex).test(signer.subjectAlternativeName)) {
      throw Object.assign(new Error("certificate identity error"), {
        code: "UNTRUSTED_SIGNER_ERROR",
      });
    }
    return signer;
  };
  return { verifyBundle, seen };
}

// Registry ECDSA key + signatures, generated once per test run.
const REGISTRY_KEY = generateKeyPairSync("ec", { namedCurve: "P-256" });
const REGISTRY_KEYS = [
  {
    keyid: "SHA256:test",
    key: REGISTRY_KEY.publicKey.export({ format: "der", type: "spki" }).toString("base64"),
  },
];
function signPackages(world) {
  world.registryKeys ??= REGISTRY_KEYS;
  for (const [name, pkg] of Object.entries(world.packages)) {
    if (pkg.signatures) continue;
    const sig = cryptoSign(
      "sha256",
      Buffer.from(`${name}@${VERSION}:${pkg.integrity}`),
      REGISTRY_KEY.privateKey,
    );
    pkg.signatures = [{ keyid: "SHA256:test", sig: sig.toString("base64") }];
  }
}

async function run(tweak, options = {}, extra = {}) {
  const world = makeWorld(tweak);
  signPackages(world);
  extra.afterBuild?.(world);
  const { fetchImpl, calls } = makeFetch(world);
  const verifier = fakeVerifier(world);
  const http = createHttpClient({ fetchImpl, retries: extra.retries ?? 1, sleep: async () => {} });
  const report = await verifyReleaseDelivery(
    { version: VERSION, releaseRunId: RELEASE_RUN, installerRunId: INSTALLER_RUN, ...options },
    {
      http,
      verifyBundle: "verifyBundle" in extra ? extra.verifyBundle : verifier.verifyBundle,
      tufCachePath: "/nonexistent-test-tuf",
      control: { commit: "c0ffee", dirty: false, provenanceVerifier: "sigstore@test" },
      now: () => Date.parse("2026-11-06T10:00:00Z"),
    },
  );
  return { report, calls, world, verifier };
}

function check(report, section, id) {
  return report.sections[section].checks.find((candidate) => candidate.id === id);
}

// ── tests ───────────────────────────────────────────────────────────────────

describe("complete normal release", () => {
  it("verifies every section, all eleven packages and four WASM files", async () => {
    const { report, verifier } = await run();
    expect(report.unverified).toEqual([]);
    expect(report.verdict).toBe(STATUS.VERIFIED);
    expect(report.exitCode).toBe(EXIT.verified);
    expect(report.release).toMatchObject({ commit: COMMIT, deliveryPath: "normal" });
    const packageChecks = report.sections.npm.checks.filter((c) => c.id.startsWith("package:"));
    expect(packageChecks.map((c) => c.id.slice(8))).toEqual(PUBLISHED_PACKAGES);
    expect(PUBLISHED_PACKAGES).toHaveLength(11);
    for (const name of Object.keys(PLATFORM_CARRIERS)) {
      const pkg = check(report, "npm", `package:${name}`).evidence;
      expect(pkg.provenance).toMatchObject(
        OWNER_MAC_CARRIERS[name]
          ? { attestationPresent: false, expected: "none" }
          : {
              attestationPresent: true,
              signatureVerified: true,
              identityMatched: true,
              subjectBound: true,
            },
      );
      expect(pkg.contents.binary).toBeTruthy();
    }
    // Nine attested packages; the two owner-Mac Darwin carriers carry none.
    expect(verifier.seen).toHaveLength(9);
    expect(verifier.seen[0].policy.identityRegex).toBe(`^${escapeRegExp(IDENTITY)}$`);
    expect(verifier.seen[0].policy.issuer).toBe("https://token.actions.githubusercontent.com");
    expect(check(report, "wasm", "packed-artifacts").evidence.artifacts).toHaveLength(4);
    expect(
      report.sections.archives.checks.filter((c) => c.status === STATUS.VERIFIED),
    ).toHaveLength(5);
    expect(summarize(report)).toContain("VERIFIED (exit 0)");
  });

  it("performs only GET requests and never targets a mutation endpoint", async () => {
    const { calls } = await run();
    expect(calls.length).toBeGreaterThan(30);
    expect(new Set(calls.map((c) => c.method))).toEqual(new Set(["GET"]));
    expect(calls.every((c) => c.body === undefined)).toBe(true);
    expect(
      calls.some((c) =>
        /uploads\.github\.com|\/dispatches|\/rerun|\/issues|dist-tags\/(?!$)/.test(c.url),
      ),
    ).toBe(false);
  });

  it("peels an annotated tag to its commit", async () => {
    const { report } = await run((w) => {
      w.tagRef = { ref: `refs/tags/${TAG}`, object: { sha: "a".repeat(40), type: "tag" } };
      w.tagObjects["a".repeat(40)] = { object: { sha: COMMIT, type: "commit" } };
    });
    expect(check(report, "identity", "tag").evidence).toMatchObject({
      commit: COMMIT,
      annotated: true,
    });
    expect(report.verdict).toBe(STATUS.VERIFIED);
  });
});

describe("release and asset delivery failures", () => {
  it("fails an asset-free release", async () => {
    const { report } = await run((w) => {
      w.archives = {};
    });
    expect(report.verdict).toBe(STATUS.FAILED);
    expect(report.sections.archives.checks.filter((c) => c.status === STATUS.FAILED)).toHaveLength(
      5,
    );
    expect(check(report, "wasm", "release-manifest").status).toBe(STATUS.FAILED);
  });

  it("fails a missing platform archive and names it", async () => {
    const { report } = await run((w) => {
      delete w.archives[`zfb-${VERSION}-x86_64-apple-darwin.tar.gz`];
    });
    const failed = check(report, "archives", "archive:x86_64-apple-darwin");
    expect(failed.status).toBe(STATUS.FAILED);
    expect(failed.reason).toContain("found 0 and 1");
    expect(check(report, "archives", "archive:aarch64-apple-darwin").status).toBe(STATUS.VERIFIED);
  });

  it("fails a checksum mismatch computed from the downloaded bytes", async () => {
    const { report } = await run((w) => {
      const name = `zfb-${VERSION}-x86_64-pc-windows-msvc.zip`;
      w.archives[`${name}.sha256`] = Buffer.from(`${"0".repeat(64)}  ${name}`);
    });
    expect(check(report, "archives", "archive:x86_64-pc-windows-msvc").reason).toContain(
      "differs from its checksum file",
    );
    expect(report.exitCode).toBe(EXIT.failed);
  });

  it("fails a checksum file that names a different archive", async () => {
    const { report } = await run((w) => {
      const name = `zfb-${VERSION}-aarch64-unknown-linux-gnu.tar.gz`;
      w.archives[`${name}.sha256`] = Buffer.from(`${sha256(w.archives[name])}  other.tar.gz\n`);
    });
    expect(check(report, "archives", "archive:aarch64-unknown-linux-gnu").reason).toContain(
      "names other.tar.gz",
    );
  });

  it("fails a draft or mismatched Release", async () => {
    const { report } = await run((w) => {
      w.release.draft = true;
      w.release.target_commitish = "b".repeat(40);
    });
    const release = check(report, "identity", "release");
    expect(release.status).toBe(STATUS.FAILED);
    expect(release.reason).toMatch(/draft.*target_commitish/s);
  });

  it("fails when the source tree's version disagrees", async () => {
    const { report } = await run((w) => {
      w.sourceVersion = "5.2.1";
    });
    expect(check(report, "identity", "source-version").status).toBe(STATUS.FAILED);
  });
});

describe("npm integrity and provenance", () => {
  it("fails partial npm publication", async () => {
    const { report } = await run((w) => {
      w.packages["@takazudo/zfb-win32-x64-msvc"].unpublished = true;
      w.packages["create-zfb"].unpublished = true;
    });
    expect(check(report, "npm", "package:@takazudo/zfb-win32-x64-msvc").status).toBe(STATUS.FAILED);
    expect(check(report, "npm", "package:create-zfb").status).toBe(STATUS.FAILED);
    expect(check(report, "npm", "package:@takazudo/zfb").status).toBe(STATUS.VERIFIED);
    expect(report.verdict).toBe(STATUS.FAILED);
  });

  it("fails a tarball whose bytes do not match dist.integrity", async () => {
    const { report } = await run(
      undefined,
      {},
      {
        afterBuild: (w) => {
          w.packages["@takazudo/zfb-darwin-x64"].servedBytes = tgz({
            "package.json": { data: "{}" },
          });
        },
      },
    );
    const pkg = check(report, "npm", "package:@takazudo/zfb-darwin-x64");
    expect(pkg.status).toBe(STATUS.FAILED);
    expect(pkg.evidence.checks.tarball.status).toBe(STATUS.FAILED);
  });

  it("fails a carrier whose binary lost its exec bit", async () => {
    const { report } = await run((w) => {
      w.packages["@takazudo/zfb-linux-arm64-gnu"].files.zfb.mode = 0o644;
    });
    expect(check(report, "npm", "package:@takazudo/zfb-linux-arm64-gnu").reason).toContain(
      "not executable",
    );
  });

  it("fails an invalid registry signature", async () => {
    const { report } = await run((w) => {
      w.packages["@takazudo/zfb-slugify"].signatures = [
        { keyid: "SHA256:test", sig: Buffer.from("bad").toString("base64") },
      ];
    });
    expect(
      check(report, "npm", "package:@takazudo/zfb-slugify").evidence.checks["registry-signature"]
        .status,
    ).toBe(STATUS.FAILED);
  });

  it("fails a CI-built package that has no provenance attestation", async () => {
    const { report } = await run((w) => {
      w.packages["@takazudo/zfb-linux-x64-gnu"].noAttestation = true;
    });
    expect(check(report, "npm", "package:@takazudo/zfb-linux-x64-gnu").reason).toContain(
      "no SLSA provenance",
    );
  });

  it("verifies owner-Mac Darwin carriers by Release bytes, without provenance (#4690)", async () => {
    const { report } = await run();
    for (const name of Object.keys(OWNER_MAC_CARRIERS)) {
      const pkg = check(report, "npm", `package:${name}`);
      expect(pkg.status).toBe(STATUS.VERIFIED);
      expect(pkg.evidence.checks["darwin-identity"].status).toBe(STATUS.VERIFIED);
      expect(pkg.evidence.checks.provenance.status).toBe(STATUS.VERIFIED);
    }
  });

  it("fails an owner-Mac Darwin carrier that carries an attestation", async () => {
    const { report } = await run((w) => {
      w.packages["@takazudo/zfb-darwin-arm64"].noAttestation = false;
    });
    expect(check(report, "npm", "package:@takazudo/zfb-darwin-arm64").reason).toContain(
      "carries an attestation",
    );
  });

  it("fails an owner-Mac Darwin carrier whose binary differs from the Release archive", async () => {
    const { report } = await run((w) => {
      w.packages["@takazudo/zfb-darwin-x64"].files.zfb.data = "different bytes";
    });
    expect(check(report, "npm", "package:@takazudo/zfb-darwin-x64").reason).toContain(
      "differ from the Release Darwin assets",
    );
  });

  it("fails a Darwin archive whose addon asset does not match its checksum", async () => {
    const { report } = await run((w) => {
      w.archives[`zfb-css-${VERSION}-aarch64-apple-darwin.node`] = Buffer.from("tampered");
    });
    expect(check(report, "archives", "archive:aarch64-apple-darwin").reason).toContain(
      "do not match its .sha256",
    );
    expect(
      check(report, "npm", "package:@takazudo/zfb-darwin-arm64").evidence.checks["darwin-identity"]
        .status,
    ).toBe(STATUS.INCOMPLETE);
  });

  it.each([
    [
      "wrong source commit",
      (s) =>
        (s.predicate.buildDefinition.resolvedDependencies[0].digest.gitCommit = "c".repeat(40)),
      /resolved source commit/,
    ],
    [
      "wrong workflow",
      (s) =>
        (s.predicate.buildDefinition.externalParameters.workflow.path =
          ".github/workflows/other.yml"),
      /workflow path/,
    ],
    [
      "wrong subject digest",
      (s) => (s.subject[0].digest.sha512 = "0".repeat(128)),
      /subject sha512/,
    ],
    ["wrong subject name", (s) => (s.subject[0].name = "pkg:npm/evil@5.2.2"), /subject name/],
    [
      "unrelated run",
      (s) =>
        (s.predicate.runDetails.metadata.invocationId = `https://github.com/${REPO}/actions/runs/1/attempts/1`),
      /invocation run 1/,
    ],
  ])("fails an attestation with a %s", async (_label, mutate, pattern) => {
    const { report } = await run((w) => {
      w.statementTweak = (name, statement) => {
        if (name === "@takazudo/zfb-runtime") mutate(statement);
      };
    });
    const pkg = check(report, "npm", "package:@takazudo/zfb-runtime");
    expect(pkg.status).toBe(STATUS.FAILED);
    expect(pkg.evidence.provenance.signatureVerified).toBe(true);
    expect(pkg.evidence.provenance.identityMatched).toBe(false);
    expect(pkg.reason).toMatch(pattern);
  });

  it("fails when the signing certificate's extensions name another commit", async () => {
    const { report } = await run((w) => {
      w.signerTweak = (name, signer) => {
        if (name === "@takazudo/zfb")
          signer.oids[FULCIO_OIDS.sourceRepositoryDigest] = "d".repeat(40);
      };
    });
    expect(check(report, "npm", "package:@takazudo/zfb").reason).toContain(
      FULCIO_OIDS.sourceRepositoryDigest,
    );
  });

  it("fails a cryptographic verification failure", async () => {
    const { report } = await run((w) => {
      w.signerTweak = (name) =>
        name === "@takazudo/zfb-md-wasm"
          ? Object.assign(new Error("signature verification failed"), { code: "SIGNATURE_ERROR" })
          : undefined;
    });
    const pkg = check(report, "npm", "package:@takazudo/zfb-md-wasm");
    expect(pkg.status).toBe(STATUS.FAILED);
    expect(pkg.evidence.provenance).toMatchObject({
      attestationPresent: true,
      signatureVerified: false,
    });
  });

  it("reports unavailable verification (TUF/network) as incomplete, never passed", async () => {
    const { report } = await run((w) => {
      w.signerTweak = () =>
        Object.assign(new Error("tuf fetch failed"), { code: "TUF_FIND_TARGET_ERROR" });
    });
    expect(report.verdict).toBe(STATUS.INCOMPLETE);
    expect(report.exitCode).toBe(EXIT.incomplete);
  });

  it("reports a missing verifier as incomplete", async () => {
    const { report } = await run(undefined, {}, { verifyBundle: undefined });
    expect(check(report, "npm", "package:create-zfb").reason).toContain(
      "no cryptographic provenance verifier",
    );
    expect(report.verdict).toBe(STATUS.INCOMPLETE);
  });
});

describe("published WASM bytes", () => {
  it("fails WASM bytes that differ from the packed manifest", async () => {
    const { report } = await run((w) => {
      w.packages["@takazudo/zfb-md-wasm"].files[
        "dist/wasm-render/zfb_md_wasm_render_bg.wasm"
      ].data = Buffer.from("tampered");
    });
    expect(check(report, "wasm", "packed-artifacts").reason).toContain("./render");
  });

  it("fails a Release manifest asset that differs from the packed manifest", async () => {
    const { report } = await run((w) => {
      w.archives[`zfb-md-wasm-${VERSION}-shipped-artifacts.json`] = Buffer.from("{}\n");
    });
    expect(check(report, "wasm", "release-manifest").status).toBe(STATUS.FAILED);
    expect(check(report, "wasm", "packed-artifacts").status).toBe(STATUS.VERIFIED);
  });
});

describe("workflow jobs, steps and attempts", () => {
  it.each([
    ["failure", STATUS.FAILED],
    ["cancelled", STATUS.FAILED],
    ["skipped", STATUS.FAILED],
  ])("treats a required %s publish job as %s", async (conclusion, status) => {
    const { report } = await run((w) => {
      w.releaseJobs.find((job) => job.name === "publish").conclusion = conclusion;
    });
    expect(check(report, "workflow", "job:publish").status).toBe(status);
  });

  it("treats a pending job as incomplete", async () => {
    const { report } = await run((w) => {
      Object.assign(
        w.releaseJobs.find((job) => job.name === "Upload GitHub Release assets"),
        { status: "in_progress", conclusion: null },
      );
      Object.assign(w.releaseRun, { status: "in_progress", conclusion: null });
    });
    expect(check(report, "workflow", "job:release-assets").status).toBe(STATUS.INCOMPLETE);
    expect(check(report, "workflow", "release-run").status).toBe(STATUS.INCOMPLETE);
  });

  it("fails a green job whose upload step was skipped", async () => {
    const { report } = await run((w) => {
      const job = w.releaseJobs.find(
        (candidate) => candidate.name === "Upload GitHub Release assets",
      );
      job.steps.find(
        (s) => s.name === "Upload archives and checksums to GitHub Release",
      ).conclusion = "skipped";
    });
    expect(check(report, "workflow", "job:release-assets").reason).toContain("was skipped");
  });

  it("classifies a successful dry run as delivering nothing", async () => {
    const { report } = await run((w) => {
      for (const job of w.releaseJobs) {
        for (const s of job.steps) {
          if (s.name.startsWith("DRY RUN")) s.conclusion = "success";
          if (
            [
              "Upload archives and checksums to GitHub Release",
              "Publish to npm",
              "Verify published @takazudo/zfb-md-wasm and upload digest asset",
            ].includes(s.name)
          ) {
            s.conclusion = "skipped";
          }
        }
      }
      w.releaseRun.event = "workflow_dispatch";
    });
    expect(report.release.deliveryPath).toBe("dry-run");
    expect(check(report, "workflow", "delivery-path").status).toBe(STATUS.FAILED);
    expect(check(report, "workflow", "job:publish").status).toBe(STATUS.FAILED);
    expect(report.verdict).toBe(STATUS.FAILED);
  });

  it("classifies the recovery path as unsupported, not normal", async () => {
    const { report } = await run((w) => {
      const publish = w.releaseJobs.find((job) => job.name === "publish");
      publish.steps.find((s) => s.name === "Publish to npm (recovery — no provenance)").conclusion =
        "success";
      publish.steps.find((s) => s.name === "Publish to npm").conclusion = "skipped";
    });
    expect(report.release.deliveryPath).toBe("recovery");
    expect(check(report, "workflow", "delivery-path").status).toBe(STATUS.INCOMPLETE);
    expect(report.verdict).toBe(STATUS.FAILED);
  });

  it("classifies a pre-#4690 run with a hosted Darwin build leg as legacy", async () => {
    const { report } = await run((w) => {
      w.releaseJobs.push({
        id: 9300,
        name: "Build x86_64-apple-darwin",
        run_attempt: 1,
        status: "completed",
        conclusion: "success",
        steps: [step("Create archive (Unix tar.gz)")],
      });
    });
    expect(report.release.deliveryPath).toBe("legacy-hosted-darwin");
    expect(check(report, "workflow", "delivery-path").status).toBe(STATUS.INCOMPLETE);
  });

  it("classifies a finalize-only run as unsupported, not normal", async () => {
    const { report } = await run((w) => {
      w.releaseJobs.push({
        id: 9301,
        name: "Finalize published release",
        run_attempt: 1,
        status: "completed",
        conclusion: "success",
        steps: [],
      });
    });
    expect(report.release.deliveryPath).toBe("finalize");
  });

  it("accepts a partial rerun that carries earlier successes forward", async () => {
    const { report } = await run((w) => {
      w.releaseRun.run_attempt = 2;
      w.attemptRuns["1"] = { status: "completed", conclusion: "failure" };
      const publish = w.releaseJobs.find((job) => job.name === "publish");
      const failed = {
        ...structuredClone(publish),
        id: 9001,
        run_attempt: 1,
        conclusion: "failure",
      };
      failed.steps.find((s) => s.name === "Publish to npm").conclusion = "failure";
      Object.assign(publish, { id: 9002, run_attempt: 2 });
      w.releaseJobs.push(failed);
      // Some packages were attested by the failed first attempt, the rest by the rerun.
      w.attestationAttempt = (name) => (PUBLISHED_PACKAGES.indexOf(name) < 5 ? 1 : 2);
    });
    expect(report.verdict).toBe(STATUS.VERIFIED);
    const job = check(report, "workflow", "job:publish").evidence;
    expect(job.effective.attempt).toBe(2);
    expect(job.history.map((h) => [h.attempt, h.conclusion])).toEqual([
      [1, "failure"],
      [2, "success"],
    ]);
    expect(check(report, "workflow", "eligible-publish-attempts").evidence.attempts).toEqual([
      1, 2,
    ]);
    expect(check(report, "workflow", "attempts").evidence.attempts[0]).toMatchObject({
      attempt: 1,
      conclusion: "failure",
    });
  });

  it("does not accept an earlier success over a later failure", async () => {
    const { report } = await run((w) => {
      w.releaseRun.run_attempt = 2;
      w.releaseRun.conclusion = "failure";
      const build = w.releaseJobs.find((job) => job.name === "Build aarch64-unknown-linux-gnu");
      w.releaseJobs.push({
        ...structuredClone(build),
        id: 9100,
        run_attempt: 2,
        conclusion: "failure",
      });
    });
    const job = check(report, "workflow", "job:build:aarch64-unknown-linux-gnu");
    expect(job.status).toBe(STATUS.FAILED);
    expect(job.evidence.effective.attempt).toBe(2);
  });

  it("rejects an attestation from an attempt whose publish step never ran", async () => {
    const { report } = await run((w) => {
      w.attestationAttempt = () => 3;
    });
    expect(check(report, "npm", "package:@takazudo/zfb").reason).toContain(
      "not an eligible publish attempt",
    );
  });

  it("fails a run that is not the release workflow for this tag", async () => {
    const { report } = await run((w) => {
      w.releaseRun.path = ".github/workflows/health.yml";
      w.releaseRun.head_branch = "main";
    });
    expect(check(report, "workflow", "release-run").reason).toMatch(/workflow path.*head_branch/s);
  });
});

describe("exact-version consumption", () => {
  it("accepts a valid installer run whose own branch is main", async () => {
    const { report } = await run();
    const runCheck = check(report, "consumption", "installer-run");
    expect(runCheck.status).toBe(STATUS.VERIFIED);
    expect(runCheck.evidence.head_branch).toBe("main");
    const evidence = check(report, "consumption", "installer-evidence").evidence;
    expect(evidence.installScriptRef).toBe("main");
    expect(evidence.note).toContain("not immutable installer-script bytes");
  });

  it("fails a green downstream run whose installer job was skipped", async () => {
    const { report } = await run((w) => {
      w.installerJobs[0].conclusion = "skipped";
      w.installerJobs[0].steps = [];
    });
    expect(check(report, "consumption", "installer-run").status).toBe(STATUS.VERIFIED);
    expect(check(report, "consumption", "job:installer").status).toBe(STATUS.FAILED);
    expect(report.verdict).toBe(STATUS.FAILED);
  });

  it("fails installer evidence for another version", async () => {
    const { report } = await run((w) => {
      w.logs[5000] = INSTALLER_LOG.replaceAll(VERSION, "5.2.1");
    });
    expect(check(report, "consumption", "installer-evidence").reason).toContain("installedVersion");
  });

  it("treats an expired clean-room log as incomplete", async () => {
    const { report } = await run((w) => {
      delete w.logs[jobIdOf(w.releaseJobs, "smoke-clean-room")];
    });
    expect(check(report, "consumption", "release-clean-room").status).toBe(STATUS.INCOMPLETE);
    expect(report.verdict).toBe(STATUS.INCOMPLETE);
  });

  it("fails a run from the wrong workflow or event", async () => {
    const { report } = await run((w) => {
      w.installerRun.event = "push";
    });
    expect(check(report, "consumption", "installer-run").reason).toContain("not workflow_run");
  });
});

describe("mutable channels", () => {
  it("keeps a historical release verified after channels advance", async () => {
    const { report } = await run((w) => {
      w.latest = { tag_name: "v5.3.0" };
      w.distTags = () => ({ latest: "5.3.0" });
      w.formula = advancedFormula(w, "5.3.0");
    });
    expect(report.verdict).toBe(STATUS.VERIFIED);
    expect(check(report, "channels", "homebrew")).toMatchObject({
      status: STATUS.OBSERVED,
      evidence: { version: "5.3.0", matchesThisReleaseArchives: false },
    });
    expect(check(report, "channels", "github-latest")).toMatchObject({
      status: STATUS.OBSERVED,
      evidence: { tag_name: "v5.3.0" },
    });
    expect(report.sections.channels.status).toBe(STATUS.NOT_APPLICABLE);
  });

  it("enforces channels only when expectations are requested", async () => {
    const { report } = await run(
      (w) => {
        w.latest = { tag_name: "v5.3.0" };
        w.distTags = (name) => ({ latest: name === "create-zfb" ? "5.3.0" : VERSION });
      },
      { expectGithubLatest: true, expectNpmLatest: true, expectHomebrew: true },
    );
    expect(check(report, "channels", "github-latest").status).toBe(STATUS.FAILED);
    expect(check(report, "channels", "npm-dist-tags").status).toBe(STATUS.FAILED);
    expect(check(report, "channels", "homebrew").status).toBe(STATUS.VERIFIED);
    expect(report.verdict).toBe(STATUS.FAILED);
  });

  it("does not let a channel at this version stand in for delivery", async () => {
    const { report } = await run(
      (w) => {
        delete w.archives[`zfb-${VERSION}-x86_64-unknown-linux-gnu.tar.gz.sha256`];
      },
      { expectGithubLatest: true, expectNpmLatest: true },
    );
    expect(check(report, "channels", "github-latest").status).toBe(STATUS.VERIFIED);
    expect(report.verdict).toBe(STATUS.FAILED);
  });
});

describe("HTTP behavior", () => {
  it("follows job pagination to the reported total", async () => {
    const { report, calls } = await run((w) => {
      w.jobsPageSize = 4;
    });
    expect(report.verdict).toBe(STATUS.VERIFIED);
    // 14 release jobs at 4 per page.
    expect(calls.filter((c) => c.url.includes(`/runs/${RELEASE_RUN}/jobs`)).length).toBe(4);
  });

  it("reports pagination that never reaches total_count as incomplete", async () => {
    const { report } = await run((w) => {
      w.jobsTotalOverride = 500;
    });
    expect(check(report, "workflow", "jobs").status).toBe(STATUS.INCOMPLETE);
    expect(check(report, "workflow", "jobs").reason).toContain("pagination incomplete");
  });

  it("does not convert a non-404 registry error into not-found or success", async () => {
    const { report } = await run(
      undefined,
      {},
      {
        afterBuild: (w) => {
          w.overrides.set(
            `${REG}/@takazudo%2fzfb/${VERSION}`,
            () => new Response("boom", { status: 503 }),
          );
        },
      },
    );
    const pkg = check(report, "npm", "package:@takazudo/zfb");
    expect(pkg.status).toBe(STATUS.INCOMPLETE);
    expect(pkg.evidence.checks.metadata.error).toMatchObject({ kind: "server", status: 503 });
  });

  it("keeps a definite failure visible next to incomplete evidence", async () => {
    const { report } = await run(
      (w) => {
        w.packages["create-zfb"].unpublished = true;
      },
      {},
      {
        afterBuild: (w) => {
          w.overrides.set(
            `${API}/actions/runs/${INSTALLER_RUN}`,
            () => new Response("{}", { status: 500 }),
          );
        },
      },
    );
    expect(check(report, "consumption", "installer-run").status).toBe(STATUS.INCOMPLETE);
    expect(report.verdict).toBe(STATUS.FAILED);
    expect(report.exitCode).toBe(EXIT.failed);
  });

  it("classifies auth, rate limit, not found, malformed and network errors", async () => {
    const responses = {
      "https://x.test/auth": () => new Response("", { status: 401 }),
      "https://x.test/rate": () =>
        new Response("", {
          status: 403,
          headers: { "x-ratelimit-remaining": "0", "x-ratelimit-reset": "0" },
        }),
      "https://x.test/forbidden": () => new Response("", { status: 403 }),
      "https://x.test/missing": () => new Response("", { status: 404 }),
      "https://x.test/malformed": () => new Response("not json"),
      "https://x.test/network": () => {
        throw new TypeError("fetch failed");
      },
    };
    let count = 0;
    const http = createHttpClient({
      fetchImpl: async (url) => {
        count += 1;
        return responses[url]();
      },
      retries: 2,
      sleep: async () => {},
    });
    const kinds = {};
    for (const url of Object.keys(responses)) {
      kinds[url.slice("https://x.test/".length)] = await http
        .getJson(url)
        .catch((error) => error.kind);
    }
    expect(kinds).toEqual({
      auth: "auth",
      rate: "rate-limit",
      forbidden: "forbidden",
      missing: "not-found",
      malformed: "malformed",
      network: "network",
    });
    // rate and network are retried (2 retries each); the rest are terminal.
    expect(count).toBe(1 + 3 + 1 + 1 + 1 + 3);
  });

  it("gives up after bounded retries and refuses an unbounded rate-limit wait", async () => {
    const waits = [];
    const http = createHttpClient({
      fetchImpl: async () => new Response("", { status: 429, headers: { "retry-after": "1" } }),
      retries: 2,
      sleep: async (ms) => waits.push(ms),
    });
    await expect(http.getJson("https://x.test/a")).rejects.toMatchObject({ kind: "rate-limit" });
    expect(waits).toEqual([1000, 1000]);
    const slow = createHttpClient({
      fetchImpl: async () => new Response("", { status: 429, headers: { "retry-after": "99999" } }),
      retries: 5,
      sleep: async () => {
        throw new Error("must not sleep");
      },
    });
    await expect(slow.getJson("https://x.test/b")).rejects.toThrow(/exceeds the bound/);
  });

  it("enforces the response size bound", async () => {
    const http = createHttpClient({
      fetchImpl: async () => new Response(Buffer.alloc(2048)),
      retries: 0,
    });
    await expect(http.getBytes("https://x.test/big", { maxBytes: 1024 })).rejects.toMatchObject({
      kind: "too-large",
    });
  });

  it("sends the GitHub token only to api.github.com", async () => {
    const seen = [];
    const http = createHttpClient({
      token: "secret",
      fetchImpl: async (url, init) => {
        seen.push([new URL(url).host, init.headers.authorization]);
        return new Response("{}");
      },
    });
    await http.getJson("https://api.github.com/x");
    await http.getJson("https://registry.npmjs.org/x");
    await http.getJson("https://raw.githubusercontent.com/x");
    expect(seen).toEqual([
      ["api.github.com", "Bearer secret"],
      ["registry.npmjs.org", undefined],
      ["raw.githubusercontent.com", undefined],
    ]);
  });
});

describe("helpers and CLI", () => {
  it("parses the documented CLI and rejects bad input", () => {
    expect(
      parseArgs([
        "--version",
        "5.2.2",
        "--release-run-id",
        "1",
        "--installer-run-id",
        "2",
        "--json",
        "r.json",
        "--expect-github-latest",
      ]),
    ).toMatchObject({
      version: "5.2.2",
      releaseRunId: "1",
      installerRunId: "2",
      json: "r.json",
      expectGithubLatest: true,
    });
    expect(() => parseArgs(["--version", "5.2.2", "--release-run-id", "1"])).toThrow(
      /installer-run-id/,
    );
    expect(() =>
      parseArgs(["--version", "latest", "--release-run-id", "1", "--installer-run-id", "2"]),
    ).toThrow(/invalid version/);
    expect(() =>
      parseArgs(["--version", "5.2.2", "--release-run-id", "x", "--installer-run-id", "2"]),
    ).toThrow(/numeric/);
    expect(() => parseArgs(["--upload"])).toThrow(/unknown argument/);
  });

  it("escapes and anchors the certificate identity", () => {
    const regex = new RegExp(`^${escapeRegExp(IDENTITY)}$`);
    expect(regex.test(IDENTITY)).toBe(true);
    expect(regex.test(`${IDENTITY}x`)).toBe(false);
    expect(regex.test(IDENTITY.replace(".github", "Xgithub"))).toBe(false);
  });

  it("builds npm purls and decodes Fulcio extension values", () => {
    expect(npmPurl("@takazudo/zfb", "5.2.2")).toBe("pkg:npm/%40takazudo/zfb@5.2.2");
    expect(npmPurl("create-zfb", "5.2.2")).toBe("pkg:npm/create-zfb@5.2.2");
    expect(decodeExtensionValue(Buffer.from([0x0c, 0x07, ...Buffer.from("release")]))).toBe(
      "release",
    );
    expect(decodeExtensionValue(Buffer.from("refs/tags/v1"))).toBe("refs/tags/v1");
  });

  it("parses tar entries with mode", () => {
    const files = parseTar(gunzipSync(tgz({ zfb: { data: "x", mode: 0o755 } })));
    expect(files.get("package/zfb")).toMatchObject({ mode: 0o755, size: 1 });
  });

  it("ranks failed above incomplete above verified", () => {
    expect(combineStatus([STATUS.VERIFIED, STATUS.INCOMPLETE, STATUS.FAILED])).toBe(STATUS.FAILED);
    expect(combineStatus([STATUS.VERIFIED, STATUS.INCOMPLETE])).toBe(STATUS.INCOMPLETE);
    expect(combineStatus([STATUS.OBSERVED])).toBe(STATUS.NOT_APPLICABLE);
  });

  it("picks the highest attempt per job name", () => {
    const grouped = effectiveJobs([
      { id: 2, name: "a", run_attempt: 2, conclusion: "failure" },
      { id: 1, name: "a", run_attempt: 1, conclusion: "success" },
    ]);
    expect(grouped.get("a").effective.id).toBe(2);
  });
});

describe("review hardening", () => {
  it("fails an expected Latest when no release is marked Latest at all", async () => {
    const notFound = (w) => w.overrides.set(`${API}/releases/latest`, () => json({}, 404));
    const expected = await run(undefined, { expectGithubLatest: true }, { afterBuild: notFound });
    expect(check(expected.report, "channels", "github-latest").status).toBe(STATUS.FAILED);
    const observed = await run(undefined, {}, { afterBuild: notFound });
    expect(check(observed.report, "channels", "github-latest").status).toBe(STATUS.OBSERVED);
    expect(observed.report.verdict).toBe(STATUS.VERIFIED);
  });

  it("keeps a wrong npm latest failed when another package's read fails", async () => {
    const { report } = await run(
      (w) => {
        w.distTags = (name) => ({ latest: name === "create-zfb" ? "5.3.0" : VERSION });
      },
      { expectNpmLatest: true },
      {
        afterBuild: (w) =>
          w.overrides.set(
            `${REG}/-/package/@takazudo%2fzfb/dist-tags`,
            () => new Response("boom", { status: 503 }),
          ),
      },
    );
    const tags = check(report, "channels", "npm-dist-tags");
    expect(tags.status).toBe(STATUS.FAILED);
    expect(tags.reason).toContain("create-zfb=5.3.0");
  });

  it.each([
    [FULCIO_OIDS.runnerEnvironment, "self-hosted"],
    [FULCIO_OIDS.buildSignerDigest, "e".repeat(40)],
    [FULCIO_OIDS.sourceRepositoryIdentifier, "9999"],
    [FULCIO_OIDS.sourceRepositoryOwnerIdentifier, "9999"],
  ])("fails a certificate whose extension %s is %s", async (oid, value) => {
    const { report } = await run((w) => {
      w.signerTweak = (name, signer) => {
        if (name === "@takazudo/zfb") signer.oids[oid] = value;
      };
    });
    const pkg = check(report, "npm", "package:@takazudo/zfb");
    expect(pkg.status).toBe(STATUS.FAILED);
    expect(pkg.reason).toContain(oid);
  });

  it("does not pin repository ids taken from a run in another repository", async () => {
    const { report } = await run((w) => {
      w.releaseRun.repository = { full_name: "someone/else", id: 1, owner: { id: 2 } };
    });
    expect(check(report, "workflow", "release-run").status).toBe(STATUS.FAILED);
    // The certificate ids (1001/2002) are not compared against the foreign run's ids.
    expect(check(report, "npm", "package:@takazudo/zfb").reason ?? "").not.toContain(
      FULCIO_OIDS.sourceRepositoryIdentifier,
    );
  });

  it("fails an installer run created before the Release run existed", async () => {
    const { report } = await run((w) => {
      w.installerRun.created_at = "2026-11-05T00:00:00Z";
    });
    expect(check(report, "consumption", "installer-run").reason).toContain(
      "before the Release run existed",
    );
  });

  it("requires a whole 'checksum OK' line in the installer log", async () => {
    const { report } = await run((w) => {
      w.logs[5000] = INSTALLER_LOG.replace("checksum OK", "checksum OK? no: mismatch");
    });
    const evidence = check(report, "consumption", "installer-evidence");
    expect(evidence.status).toBe(STATUS.FAILED);
    expect(evidence.reason).toContain("checksumVerified");
  });

  it("reports installer evidence as incomplete when the tag commit is unresolved", async () => {
    const { report } = await run(
      undefined,
      {},
      {
        afterBuild: (w) =>
          w.overrides.set(
            `${API}/git/ref/tags/${TAG}`,
            () => new Response("boom", { status: 503 }),
          ),
      },
    );
    expect(check(report, "consumption", "installer-evidence").status).toBe(STATUS.INCOMPLETE);
    expect(report.verdict).not.toBe(STATUS.VERIFIED);
  });

  it("fails a required job that appears twice in one attempt", async () => {
    const { report } = await run((w) => {
      const build = w.releaseJobs.find((job) => job.name === "Build aarch64-unknown-linux-gnu");
      w.releaseJobs.push({ ...structuredClone(build), id: 9200 });
    });
    const job = check(report, "workflow", "job:build:aarch64-unknown-linux-gnu");
    expect(job.status).toBe(STATUS.FAILED);
    expect(job.reason).toContain("ambiguous");
  });

  it("bounds --concurrency at 4", () => {
    const base = ["--version", "5.2.2", "--release-run-id", "1", "--installer-run-id", "2"];
    expect(parseArgs([...base, "--concurrency", "4"]).concurrency).toBe(4);
    expect(() => parseArgs([...base, "--concurrency", "5"])).toThrow(/concurrency/);
  });
});

function advancedFormula(world, version) {
  const rows = {};
  for (const [key, target] of [
    ["macos/arm", "aarch64-apple-darwin"],
    ["macos/intel", "x86_64-apple-darwin"],
    ["linux/arm", "aarch64-unknown-linux-gnu"],
    ["linux/intel", "x86_64-unknown-linux-gnu"],
  ]) {
    rows[key] = {
      url: `https://github.com/${REPO}/releases/download/v${version}/zfb-${version}-${target}.tar.gz`,
      sha: "a".repeat(64),
    };
  }
  return generatorFormula(world, { version, rows });
}

describe("Homebrew formula binding (--expect-homebrew)", () => {
  const T = {
    darwinArm: "aarch64-apple-darwin",
    darwinIntel: "x86_64-apple-darwin",
    linuxArm: "aarch64-unknown-linux-gnu",
    linuxIntel: "x86_64-unknown-linux-gnu",
  };
  const urlOf = (target) => `${RELEASE_DOWNLOAD}/zfb-${VERSION}-${target}.tar.gz`;
  const pairOf = (world, target) => ({ url: urlOf(target), sha: archiveSha(world, target) });
  const runBrew = (tweak, options = {}) => run(tweak, { expectHomebrew: true, ...options });
  const brew = (report) => check(report, "channels", "homebrew");

  it("verifies a correct generator-shaped formula with per-target evidence", async () => {
    const { report, world } = await runBrew(() => {});
    expect(brew(report).status).toBe(STATUS.VERIFIED);
    expect(report.verdict).toBe(STATUS.VERIFIED);
    expect(report.exitCode).toBe(EXIT.verified);
    expect(brew(report).evidence.rows).toEqual(
      [T.darwinArm, T.darwinIntel, T.linuxArm, T.linuxIntel].map((target) =>
        expect.objectContaining({
          target,
          matches: true,
          expected: {
            url: urlOf(target),
            sha256: archiveSha(world, target),
            archiveStatus: STATUS.VERIFIED,
          },
          observed: { url: urlOf(target), sha256: archiveSha(world, target) },
        }),
      ),
    );
  });

  const falsePositives = [
    [
      "swapped Darwin/Linux ARM64 rows",
      (w) => ({ "macos/arm": pairOf(w, T.linuxArm), "linux/arm": pairOf(w, T.darwinArm) }),
    ],
    [
      "Darwin ARM64 reused in all four branches",
      (w) => {
        const pair = pairOf(w, T.darwinArm);
        return { "macos/intel": pair, "linux/arm": pair, "linux/intel": pair };
      },
    ],
    [
      "the Windows ZIP reused in all four branches",
      (w) => {
        const pair = {
          url: `${RELEASE_DOWNLOAD}/zfb-${VERSION}-x86_64-pc-windows-msvc.zip`,
          sha: archiveSha(w, "x86_64-pc-windows-msvc"),
        };
        return { "macos/arm": pair, "macos/intel": pair, "linux/arm": pair, "linux/intel": pair };
      },
    ],
    [
      "stale v0.0.1 URLs with the expected hashes",
      (w) =>
        Object.fromEntries(
          [
            ["macos/arm", T.darwinArm],
            ["macos/intel", T.darwinIntel],
            ["linux/arm", T.linuxArm],
            ["linux/intel", T.linuxIntel],
          ].map(([key, target]) => [
            key,
            {
              url: `https://github.com/${REPO}/releases/download/v0.0.1/zfb-0.0.1-${target}.tar.gz`,
              sha: archiveSha(w, target),
            },
          ]),
        ),
    ],
  ];

  it.each(falsePositives)("fails %s", async (_label, rowsFor) => {
    const { report } = await runBrew((w) => {
      w.formula = generatorFormula(w, { rows: rowsFor(w) });
    });
    expect(brew(report).status).toBe(STATUS.FAILED);
    expect(report.verdict).toBe(STATUS.FAILED);
    expect(report.exitCode).toBe(EXIT.failed);
  });

  it("names the mismatching branch in the evidence", async () => {
    const { report } = await runBrew((w) => {
      w.formula = generatorFormula(w, {
        rows: { "macos/arm": pairOf(w, T.linuxArm), "linux/arm": pairOf(w, T.darwinArm) },
      });
    });
    expect(brew(report).reason).toContain(`on_macos/on_arm: url ${urlOf(T.linuxArm)}`);
    expect(brew(report).evidence.rows[0]).toMatchObject({
      target: T.darwinArm,
      matches: false,
      observed: { url: urlOf(T.linuxArm) },
    });
  });

  const structural = [
    [
      "a missing row",
      (f) =>
        f.replace(
          /\n {4}on_intel do\n(?: {6}.*\n){2} {4}end\n {2}end\n\n {2}def/,
          "\n  end\n\n  def",
        ),
    ],
    ["a duplicate url in one row", (f) => f.replace(/( {6}url "[^"]+"\n)/, "$1$1")],
    ["a missing sha256 field", (f) => f.replace(/ {6}sha256 "[0-9a-f]+"\n/, "")],
    ["a duplicated os block", (f) => f.replace(/( {2}on_linux do\n[\s\S]*?\n {2}end\n)/, "$1$1")],
    [
      "a url inside def install",
      (f) => f.replace('bin.install "zfb"', `bin.install "zfb"\n    url "${urlOf(T.linuxIntel)}"`),
    ],
    [
      "a url directly under on_macos",
      (f) => f.replace("  on_macos do\n", `  on_macos do\n    url "${urlOf(T.darwinArm)}"\n`),
    ],
    [
      "an unsupported mirror declaration",
      (f) => f.replace(/( {6}url "[^"]+"\n)/, `$1      mirror "${urlOf(T.darwinArm)}"\n`),
    ],
    ["a url with options", (f) => f.replace(/( {6}url "[^"]+")/, "$1, using: :curl")],
    ["a second version", (f) => f.replace(/( {2}version "[^"]+"\n)/, "$1$1")],
    ["an unbalanced block", (f) => f.replace(/\nend\n$/, "\n")],
    ["a wrong version", (f) => f.replace(`version "${VERSION}"`, 'version "5.2.1"')],
  ];

  it.each(structural)("fails %s", async (_label, edit) => {
    const { report } = await runBrew((w) => {
      const formula = edit(generatorFormula(w));
      expect(formula).not.toBe(generatorFormula(w));
      w.formula = formula;
    });
    expect(brew(report).status).toBe(STATUS.FAILED);
    expect(report.exitCode).toBe(EXIT.failed);
  });

  it("ignores comment, heredoc and embedded-string decoys", async () => {
    const { report } = await runBrew((w) => {
      const decoy = urlOf("x86_64-pc-windows-msvc");
      w.formula = generatorFormula(w)
        .replace(
          "    on_arm do\n",
          `    on_arm do\n      # url "${decoy}"\n      # sha256 "${"b".repeat(64)}"\n`,
        )
        .replace(
          '  desc "',
          `  # on_linux do\n  #   url "${decoy}"\n  # end\n  caveats "url \\"${decoy}\\" sha256"\n  desc "`,
        )
        .replace(
          '    system bin/"zfb", "--version"\n',
          `    system bin/"zfb", "--version"\n    (testpath/"note").write <<~EOS\n      url "${decoy}"\n      sha256 "${"c".repeat(64)}"\n      end\n    EOS\n`,
        );
    });
    expect(brew(report).status).toBe(STATUS.VERIFIED);
  });

  it("ends a plain <<ID heredoc only at a column-0 terminator", async () => {
    const decoy = urlOf("x86_64-pc-windows-msvc");
    const { report } = await runBrew((w) => {
      w.formula = generatorFormula(w).replace(
        '    system bin/"zfb", "--version"\n',
        `    system bin/"zfb", "--version"\n    note = <<EOS\n      EOS\n      url "${decoy}"\nEOS\n`,
      );
    });
    expect(brew(report).status).toBe(STATUS.VERIFIED);
  });

  it("accepts CRLF line endings, tab indentation and extra blank lines", async () => {
    const { report } = await runBrew((w) => {
      w.formula = generatorFormula(w)
        .replace(/^ +/gm, (spaces) => "\t".repeat(spaces.length / 2))
        .replace(/\n\n/g, "\n\n\n")
        .replace(/\n/g, "\r\n");
    });
    expect(brew(report).status).toBe(STATUS.VERIFIED);
  });

  it("does not verify a row whose archive evidence is missing", async () => {
    const { report } = await runBrew((w) => {
      delete w.archives[`zfb-${VERSION}-${T.linuxIntel}.tar.gz.sha256`];
    });
    expect(check(report, "archives", `archive:${T.linuxIntel}`).status).toBe(STATUS.FAILED);
    expect(brew(report).status).toBe(STATUS.INCOMPLETE);
    expect(brew(report).reason).toContain(`${T.linuxIntel} archive evidence is ${STATUS.FAILED}`);
    expect(brew(report).evidence.rows[3]).toMatchObject({ matches: false });
    expect(report.exitCode).toBe(EXIT.failed);
  });

  it("observes an invalid formula without failing when Homebrew is not required", async () => {
    const { report } = await run((w) => {
      w.formula = generatorFormula(w, {
        rows: { "macos/arm": pairOf(w, T.linuxArm), "linux/arm": pairOf(w, T.darwinArm) },
      });
    });
    expect(brew(report).status).toBe(STATUS.OBSERVED);
    expect(brew(report).evidence.matchesThisReleaseArchives).toBe(false);
    expect(report.verdict).toBe(STATUS.VERIFIED);
  });
});
