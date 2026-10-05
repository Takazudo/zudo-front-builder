import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import { assessUpstream } from "../../../scripts/wind-compatibility/upstream.mjs";
import {
  fromRoot,
  identity,
  makePlan,
  validateMetadata,
} from "../../../scripts/wind-compatibility/reference.mjs";

const gh = "https://api.github.com/repos/tailwindlabs/tailwindcss";
const npm = "https://registry.npmjs.org/tailwindcss";
const commits = { "4.3.1": "1".repeat(40), "4.3.2": "2".repeat(40) };
const sha1 = (bytes) => createHash("sha1").update(bytes).digest("hex");
const sha512 = (bytes) => createHash("sha512").update(bytes).digest("base64");
const blobSha = (bytes) =>
  createHash("sha1").update(`blob ${bytes.length}\0`).update(bytes).digest("hex");

function tar(entries) {
  const parts = [];
  for (const [path, value] of Object.entries(entries)) {
    const data = Buffer.from(value),
      header = Buffer.alloc(512);
    const split = path.length > 100 ? path.indexOf("/") : -1;
    header.write(split < 0 ? path : path.slice(split + 1), 0);
    if (split >= 0) header.write(path.slice(0, split), 345);
    header.write(data.length.toString(8).padStart(11, "0"), 124);
    header.write("0", 156);
    parts.push(header, data, Buffer.alloc(Math.ceil(data.length / 512) * 512 - data.length));
  }
  return gzipSync(Buffer.concat([...parts, Buffer.alloc(1024)]));
}
function fixture() {
  const source = {},
    packageTar = {},
    metadata = {},
    trees = {};
  for (const version of ["4.3.1", "4.3.2"]) {
    const commit = commits[version];
    const files = {
      "CHANGELOG.md": `# Changelog\n\n## ${version}\nComplete release note.\n`,
      "packages/tailwindcss/src/utilities.ts": `export const version = '${version}';\n`,
      "packages/tailwindcss/src/utilities.test.ts": `test('version', () => '${version}');\n`,
      "packages/tailwindcss/src/__snapshots__/utilities.test.ts.snap": `exports["version"] = '${version}';\n`,
    };
    source[version] = tar(
      Object.fromEntries(
        Object.entries(files).map(([path, body]) => [`tailwindcss-${commit}/${path}`, body]),
      ),
    );
    trees[version] = {
      sha: commit,
      truncated: false,
      tree: Object.entries(files).map(([path, body]) => ({
        path,
        type: "blob",
        sha: blobSha(Buffer.from(body)),
      })),
    };
    const archive = tar({
      "package/package.json": JSON.stringify({ name: "tailwindcss", version }),
      "package/dist/lib.mjs": `export const version='${version}';`,
    });
    packageTar[version] = archive;
    metadata[version] = {
      name: "tailwindcss",
      version,
      dist: {
        integrity: `sha512-${sha512(archive)}`,
        shasum: sha1(archive),
        tarball: `${npm}/-/tailwindcss-${version}.tgz`,
      },
      repository: { url: "https://github.com/tailwindlabs/tailwindcss.git" },
    };
  }
  const releases = {
    "4.3.1": {
      id: 431,
      tag_name: "v4.3.1",
      published_at: "2026-01-01T00:00:00Z",
      body: "Full 4.3.1 notes",
    },
    "4.3.2": {
      id: 432,
      tag_name: "v4.3.2",
      published_at: "2026-01-02T00:00:00Z",
      body: "Full 4.3.2 notes",
    },
  };
  const responses = new Map();
  for (const version of ["4.3.1", "4.3.2"]) {
    const commit = commits[version];
    responses.set(`${npm}/${version}`, metadata[version]);
    responses.set(`${npm}/-/tailwindcss-${version}.tgz`, packageTar[version]);
    responses.set(`${gh}/git/ref/tags/v${version}`, {
      ref: `refs/tags/v${version}`,
      object: { sha: commit, type: "commit" },
    });
    responses.set(`${gh}/git/trees/${commit}?recursive=1`, trees[version]);
    responses.set(
      `https://codeload.github.com/tailwindlabs/tailwindcss/tar.gz/${commit}`,
      source[version],
    );
    responses.set(`${gh}/releases/tags/v${version}`, releases[version]);
  }
  responses.set(npm, { versions: metadata });
  responses.set(`${gh}/releases?per_page=100&page=1`, [releases["4.3.2"]]);
  responses.set(`${gh}/releases?per_page=100&page=2`, []);
  const fetcher = async (url) => {
    const value = responses.get(url);
    const bytes = Buffer.isBuffer(value)
      ? value
      : Buffer.from(JSON.stringify(value ?? { message: "Not Found" }));
    const link = url.endsWith("page=1") ? `<${gh}/releases?per_page=100&page=2>; rel="next"` : null;
    return {
      ok: value !== undefined,
      status: value === undefined ? 404 : 200,
      url,
      headers: {
        get: (name) =>
          name === "link" ? link : name === "content-type" ? "application/json" : null,
      },
      arrayBuffer: async () => bytes,
    };
  };
  const state = {
    profile: {
      schemaVersion: 1,
      profileId: "wind-preset-free",
      profileVersion: 1,
      profileRevision: 1,
      referencePolicy: { initialState: { acceptedReference: null, reviewedThrough: null } },
    },
    accepted: { schemaVersion: 1, acceptedReference: null },
    reviewed: { schemaVersion: 1, reviewedThrough: null },
  };
  const hashes = { profile: "a", bootstrap: "b", accepted: "c", reviewed: "d", lock: "e" };
  const candidate = validateMetadata(metadata["4.3.2"], "4.3.2");
  const bootstrapPlan = makePlan({ candidate, state, hashes, channel: "stable", toolchain: {} });
  const accepted = {
    package: "tailwindcss",
    version: "4.3.1",
    channel: "stable",
    integrity: metadata["4.3.1"].dist.integrity,
    artifactSha256: createHash("sha256").update(packageTar["4.3.1"]).digest("hex"),
    lockfileSha256: "lock",
    toolchain: { node: "test" },
    source: { packageGitSha: null, status: "reviewed-unknown" },
    profileId: "wind-preset-free",
    profileVersion: 1,
    profileRevision: 1,
    profileDigest: "profile",
    evidenceReportId: "report",
    evidenceReportDigest: "reportdigest",
  };
  const deltaPlan = makePlan({
    candidate,
    state: { ...state, accepted: { schemaVersion: 1, acceptedReference: accepted } },
    hashes,
    channel: "stable",
    toolchain: {},
  });
  return { responses, fetcher, bootstrapPlan, deltaPlan, trees };
}

async function withCache(fn) {
  const cache = await mkdtemp(join(tmpdir(), "wind-upstream-"));
  try {
    return await fn(cache);
  } finally {
    await rm(cache, { recursive: true, force: true });
  }
}

test("real upstream schemas produce complete bootstrap and accepted-to-candidate delta", async () =>
  withCache(async (cache) => {
    const status = () =>
      execFileSync("git", ["status", "--porcelain=v1", "--untracked-files=all"], {
        cwd: fromRoot("."),
      }).toString();
    const before = { status: status(), inputs: await identity() };
    const f = fixture();
    const bootstrap = await assessUpstream(f.bootstrapPlan, cache, f.fetcher);
    assert.equal(bootstrap.status, "ready-for-comparison", bootstrap.findings.join(", "));
    assert.equal(bootstrap.sections.source.changes.length, 4);
    assert.equal(bootstrap.sections.tests.changes.length, 2);
    assert.equal(bootstrap.sections.artifacts.changes.length, 2);
    assert.equal(bootstrap.tagToNpmArtifactLink, "unverified");
    const delta = await assessUpstream(f.deltaPlan, cache, f.fetcher);
    assert.equal(delta.status, "ready-for-comparison", delta.findings.join(", "));
    assert.deepEqual(delta.sections.changelog.releaseVersions, ["4.3.2"]);
    assert.equal(delta.sections.tests.changes.length, 2);
    assert.equal(delta.sections.source.previousTagCommit, commits["4.3.1"]);
    assert.deepEqual({ status: status(), inputs: await identity() }, before);
  }));

test("source truncation, wrong blob, missing release page and tag drift remain incomplete", async () =>
  withCache(async (cache) => {
    const f = fixture(),
      treeUrl = `${gh}/git/trees/${commits["4.3.2"]}?recursive=1`;
    f.responses.set(treeUrl, { ...f.trees["4.3.2"], truncated: true });
    assert.equal((await assessUpstream(f.bootstrapPlan, cache, f.fetcher)).status, "incomplete");
    f.responses.set(treeUrl, {
      ...f.trees["4.3.2"],
      tree: f.trees["4.3.2"].tree.map((x, i) => (i ? x : { ...x, sha: "0".repeat(40) })),
    });
    assert.match(
      (await assessUpstream(f.bootstrapPlan, cache, f.fetcher)).findings[0],
      /blob mismatch/,
    );
    f.responses.set(treeUrl, f.trees["4.3.2"]);
    f.responses.delete(`${gh}/releases?per_page=100&page=2`);
    assert.equal((await assessUpstream(f.deltaPlan, cache, f.fetcher)).status, "incomplete");
    f.responses.set(
      `${gh}/releases?per_page=100&page=1`,
      Array.from({ length: 100 }, () => ({ tag_name: "v4.3.2", body: "note" })),
    );
    assert.match(
      (
        await assessUpstream(f.deltaPlan, cache, async (url) => {
          const response = await f.fetcher(url);
          if (url.endsWith("page=1"))
            response.headers = { get: (name) => (name === "link" ? null : "application/json") };
          return response;
        })
      ).findings[0],
      /last page/,
    );
    const driftPlan = {
      ...f.bootstrapPlan,
      candidate: { ...f.bootstrapPlan.candidate, source: { observedTagCommit: "f".repeat(40) } },
    };
    assert.match(
      (await assessUpstream(driftPlan, cache, f.fetcher)).findings[0],
      /tag commit changed/,
    );
    assert.match(
      (
        await assessUpstream(f.bootstrapPlan, cache, async () => {
          throw Error("offline");
        })
      ).findings[0],
      /offline/,
    );
  }));
