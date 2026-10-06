import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { gunzipSync } from "node:zlib";
import {
  acquire,
  compareVersion,
  digest,
  outsideCheckout,
  sha256,
  validateMetadata,
} from "./reference.mjs";

const gh = "https://api.github.com/repos/tailwindlabs/tailwindcss";
const npm = "https://registry.npmjs.org/tailwindcss";
const maxResponseBytes = 32 * 1024 * 1024;
const maxReleasePages = 20;
const maxVersions = 30;
const hex40 = /^[0-9a-f]{40}$/;

function gitBlobSha(content) {
  return createHash("sha1").update(`blob ${content.length}\0`).update(content).digest("hex");
}
function parsePax(content) {
  const values = {};
  const text = content.toString("utf8");
  let at = 0;
  while (at < text.length) {
    const end = text.indexOf(" ", at);
    const length = Number(text.slice(at, end));
    if (!Number.isSafeInteger(length) || length <= 0 || at + length > text.length)
      throw Error("Invalid PAX record");
    const line = text.slice(end + 1, at + length - 1);
    const equals = line.indexOf("=");
    if (equals < 0) throw Error("Invalid PAX field");
    values[line.slice(0, equals)] = line.slice(equals + 1);
    at += length;
  }
  return values;
}
export function tarFiles(gzip, prefix) {
  const tar = gunzipSync(gzip),
    files = new Map();
  let pax = {};
  for (let at = 0; at + 512 <= tar.length;) {
    const header = tar.subarray(at, at + 512);
    const name = header.toString("utf8", 0, 100).replace(/\0.*$/, "");
    if (!name) break;
    const prefixField = header.toString("utf8", 345, 500).replace(/\0.*$/, "");
    const size = parseInt(header.toString("ascii", 124, 136).replace(/\0.*$/, "").trim(), 8);
    if (!Number.isSafeInteger(size) || size < 0 || at + 512 + size > tar.length)
      throw Error("Invalid tar entry");
    const content = tar.subarray(at + 512, at + 512 + size);
    const type = String.fromCharCode(header[156]);
    if (type === "g") {
      /* codeload global PAX metadata is not a path override */
    } else if (type === "x") pax = parsePax(content);
    else {
      const path = pax.path ?? (prefixField ? `${prefixField}/${name}` : name);
      pax = {};
      if (!path.startsWith(prefix) || path.includes("..") || path.startsWith("/"))
        throw Error("Unexpected tar path");
      if (type === "0" || type === "\0") {
        const relative = path.slice(prefix.length);
        if (!relative || files.has(relative)) throw Error("Duplicate or empty tar path");
        files.set(relative, Buffer.from(content));
      } else if (type !== "5") throw Error(`Unsupported tar entry type ${type}`);
    }
    at += 512 + Math.ceil(size / 512) * 512;
  }
  return files;
}
function inventory(files) {
  return Object.fromEntries(
    [...files]
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([path, content]) => [path, { sha256: sha256(content), bytes: content.length }]),
  );
}
function diffInventory(previous, current) {
  const changes = [];
  for (const path of [...new Set([...Object.keys(previous), ...Object.keys(current)])].sort()) {
    const from = previous[path] ?? null,
      to = current[path] ?? null;
    if (from?.sha256 !== to?.sha256) changes.push({ path, before: from, after: to });
  }
  return changes;
}
function testPath(path) {
  return /(?:^|\/)(?:__tests__|__snapshots__|tests?|fixtures)(?:\/|$)|(?:\.test|\.spec)\.[^.]+$|\.snap$/.test(
    path,
  );
}

export async function assessUpstream(plan, cacheDir, fetcher = fetch) {
  const cache = await outsideCheckout(cacheDir);
  await mkdir(cache, { recursive: true });
  await outsideCheckout(cache);
  const captures = [],
    findings = [];
  async function raw(url) {
    const response = await fetcher(url, {
      headers: {
        accept: "application/vnd.github+json, application/json, application/octet-stream",
      },
    });
    if (response.url && response.url !== url) throw Error(`Unexpected redirect for ${url}`);
    if (!response.ok || response.status !== 200) throw Error(`${url}: HTTP ${response.status}`);
    if (!response.headers?.get?.("content-type"))
      throw Error(`${url}: missing response content type`);
    const bytes = Buffer.from(await response.arrayBuffer());
    if (!bytes.length || bytes.length > maxResponseBytes)
      throw Error(`${url}: empty or oversized response`);
    const hash = sha256(bytes),
      path = resolve(cache, `response-${hash}`);
    await outsideCheckout(path);
    await writeFile(path, bytes, { flag: "wx" }).catch(async (error) => {
      if (error.code !== "EEXIST" || sha256(await readFile(path)) !== hash) throw error;
    });
    captures.push({
      url,
      sha256: hash,
      bytes: bytes.length,
      cacheFile: `response-${hash}`,
      contentType: response.headers?.get?.("content-type") ?? null,
      link: response.headers?.get?.("link") ?? null,
    });
    return { bytes, response };
  }
  async function json(url) {
    const { bytes, response } = await raw(url);
    let value;
    try {
      value = JSON.parse(bytes.toString("utf8"));
    } catch {
      throw Error(`${url}: invalid JSON response`);
    }
    return { value, response };
  }
  async function packageSnapshot(version) {
    const metadata = (await json(`${npm}/${encodeURIComponent(version)}`)).value;
    const meta = validateMetadata(metadata, version);
    if (meta.tarball !== `${npm}/-/tailwindcss-${version}.tgz`)
      throw Error("Unexpected npm tarball URL");
    const { bytes } = await raw(meta.tarball);
    const acquired = await acquire(meta, cache, async () => ({
      ok: true,
      arrayBuffer: async () => bytes,
    }));
    return { meta, acquired, files: inventory(tarFiles(bytes, "package/")) };
  }
  async function sourceSnapshot(version) {
    const ref = (await json(`${gh}/git/ref/tags/v${encodeURIComponent(version)}`)).value;
    if (
      ref.ref !== `refs/tags/v${version}` ||
      !hex40.test(ref.object?.sha) ||
      ref.object.type !== "commit"
    )
      throw Error(`Unverifiable git ref for ${version}`);
    const commit = ref.object.sha;
    const tree = (await json(`${gh}/git/trees/${commit}?recursive=1`)).value;
    if (tree.truncated !== false || !Array.isArray(tree.tree) || !tree.tree.length)
      throw Error(`Incomplete source tree for ${version}`);
    const expected = new Map();
    for (const item of tree.tree)
      if (item.type === "blob") {
        if (!hex40.test(item.sha) || expected.has(item.path))
          throw Error("Invalid source tree blob");
        expected.set(item.path, item.sha);
      }
    const archiveUrl = `https://codeload.github.com/tailwindlabs/tailwindcss/tar.gz/${commit}`;
    const archive = (await raw(archiveUrl)).bytes;
    const files = tarFiles(archive, `tailwindcss-${commit}/`);
    if (files.size !== expected.size)
      throw Error(`Source archive/tree count mismatch for ${version}`);
    for (const [path, content] of files)
      if (gitBlobSha(content) !== expected.get(path))
        throw Error(`Source archive/tree blob mismatch: ${path}`);
    if (!files.has("CHANGELOG.md")) throw Error("Source CHANGELOG.md missing");
    return {
      version,
      tagCommit: commit,
      files: inventory(files),
      changelogSha256: sha256(files.get("CHANGELOG.md")),
      changelogBytes: files.get("CHANGELOG.md").length,
      treeItems: tree.tree.length,
    };
  }
  try {
    const candidate = await packageSnapshot(plan.candidate.version);
    if (
      candidate.meta.integrity !== plan.candidate.integrity ||
      candidate.meta.tarball !== plan.candidate.tarball
    )
      throw Error("Plan/npm candidate identity mismatch");
    if (
      plan.candidate.artifactSha256 &&
      candidate.acquired.sha256 !== plan.candidate.artifactSha256
    )
      throw Error("Pinned candidate artifact digest changed");
    const source = await sourceSnapshot(plan.candidate.version);
    if (
      plan.candidate.source?.observedTagCommit &&
      source.tagCommit !== plan.candidate.source.observedTagCommit
    )
      throw Error("Pinned observed tag commit changed");
    const release = (
      await json(`${gh}/releases/tags/v${encodeURIComponent(plan.candidate.version)}`)
    ).value;
    if (
      release.tag_name !== `v${plan.candidate.version}` ||
      !release.body?.trim() ||
      !release.published_at
    )
      throw Error("Candidate changelog release missing");
    let previous = null,
      previousSource = null,
      releases = [release],
      intervening = [plan.candidate.version];
    if (plan.verificationOnly) {
      if (
        !plan.previousAccepted ||
        plan.previousAccepted.version !== candidate.meta.version ||
        plan.previousAccepted.integrity !== candidate.meta.integrity ||
        plan.previousAccepted.artifactSha256 !== candidate.acquired.sha256 ||
        plan.previousAccepted.source?.observedTagCommit !== source.tagCommit
      )
        throw Error("Current accepted reference changed during assessment");
      previous = candidate;
      previousSource = source;
      const packument = (await json(npm)).value;
      const listed = validateMetadata(
        packument.versions?.[candidate.meta.version],
        candidate.meta.version,
      );
      if (
        listed.integrity !== candidate.meta.integrity ||
        listed.tarball !== candidate.meta.tarball
      )
        throw Error("Current npm catalog/reference mismatch");
      // Current verification proves this exact release again. It does not
      // invent an upgrade interval or claim that accepted advanced.
      releases = [release];
      intervening = [candidate.meta.version];
    } else if (plan.previousAccepted) {
      previous = await packageSnapshot(plan.previousAccepted.version);
      if (
        previous.meta.integrity !== plan.previousAccepted.integrity ||
        previous.acquired.sha256 !== plan.previousAccepted.artifactSha256
      )
        throw Error("Accepted/npm reference identity mismatch");
      previousSource = await sourceSnapshot(plan.previousAccepted.version);
      if (
        plan.previousAccepted.source?.observedTagCommit &&
        previousSource.tagCommit !== plan.previousAccepted.source.observedTagCommit
      )
        throw Error("Accepted observed tag commit changed");
      const packument = (await json(npm)).value;
      if (!packument.versions || typeof packument.versions !== "object")
        throw Error("npm version catalog missing");
      const listedPrevious = validateMetadata(
        packument.versions[plan.previousAccepted.version],
        plan.previousAccepted.version,
      );
      if (
        listedPrevious.integrity !== previous.meta.integrity ||
        listedPrevious.tarball !== previous.meta.tarball
      )
        throw Error("npm catalog/accepted mismatch");
      intervening = Object.keys(packument.versions)
        .filter((v) => {
          try {
            return (
              (plan.channel === "prerelease" || !v.includes("-")) &&
              compareVersion(v, plan.previousAccepted.version) > 0 &&
              compareVersion(v, plan.candidate.version) <= 0
            );
          } catch {
            return false;
          }
        })
        .sort(compareVersion);
      if (!intervening.includes(plan.candidate.version) || intervening.length > maxVersions)
        throw Error("Incomplete or excessive npm version range");
      for (const version of intervening) {
        const listed = validateMetadata(packument.versions[version], version);
        if (
          version === plan.candidate.version &&
          (listed.integrity !== candidate.meta.integrity ||
            listed.tarball !== candidate.meta.tarball)
        )
          throw Error("npm catalog/candidate mismatch");
      }
      releases = [];
      for (let page = 1; page <= maxReleasePages; page++) {
        const url = `${gh}/releases?per_page=100&page=${page}`;
        const result = await json(url);
        if (!Array.isArray(result.value)) throw Error("Invalid GitHub releases page");
        releases.push(...result.value);
        const link = result.response.headers?.get?.("link") ?? "";
        const next = /<([^>]+)>;\s*rel="next"/.exec(link)?.[1] ?? null;
        if (!next && result.value.length === 100)
          throw Error("Unverifiable GitHub releases last page");
        if (!next) break;
        if (next !== `${gh}/releases?per_page=100&page=${page + 1}`)
          throw Error("GitHub releases cursor mismatch");
        if (page === maxReleasePages) throw Error("GitHub releases pagination limit reached");
      }
      const seen = new Set();
      releases = releases
        .filter((r) => intervening.includes(r.tag_name?.replace(/^v/, "")))
        .filter((r) => {
          if (seen.has(r.tag_name)) throw Error("Duplicate GitHub release");
          seen.add(r.tag_name);
          return true;
        });
      if (releases.length !== intervening.length || releases.some((r) => !r.body?.trim()))
        throw Error("Missing changelog release in npm version range");
      const listedCandidate = releases.find((r) => r.tag_name === release.tag_name);
      if (listedCandidate?.id !== release.id || listedCandidate?.body !== release.body)
        throw Error("Candidate release changed during pagination");
    }
    const sourceChanges = diffInventory(previousSource?.files ?? {}, source.files);
    const testChanges = sourceChanges.filter((change) => testPath(change.path));
    const artifactChanges = diffInventory(previous?.files ?? {}, candidate.files);
    const report = {
      schemaVersion: 1,
      kind: "wind-reference-assessment",
      ...(plan.verificationOnly ? { verificationOnly: true } : {}),
      planId: plan.planId,
      candidate: plan.candidate,
      status: "ready-for-comparison",
      findings,
      captureIdentity: `sha256:${digest(captures)}`,
      captures,
      sections: {
        changelog: {
          complete: true,
          releaseVersions: intervening,
          releaseIds: releases.map((r) => r.id),
          releaseBodiesSha256: releases.map((r) => sha256(r.body)),
          sourceChangelogSha256: source.changelogSha256,
        },
        artifacts: {
          complete: true,
          packageVersion: candidate.meta.version,
          integrity: candidate.meta.integrity,
          tarballSha256: candidate.acquired.sha256,
          fileCount: Object.keys(candidate.files).length,
          changes: artifactChanges,
        },
        source: {
          complete: true,
          tagCommit: source.tagCommit,
          previousTagCommit: previousSource?.tagCommit ?? null,
          treeItems: source.treeItems,
          fileCount: Object.keys(source.files).length,
          changes: sourceChanges,
        },
        tests: {
          complete: true,
          sourceTagCommit: source.tagCommit,
          changedFileCount: testChanges.length,
          changes: testChanges,
        },
      },
      sourceIdentityStatus: plan.candidate.source?.status ?? "unknown",
      packageGitSha: plan.candidate.source?.packageGitSha ?? null,
      tagToNpmArtifactLink: "unverified",
      acceptedReferenceAdvanced: false,
      reviewedThroughAdvanced: false,
    };
    return report;
  } catch (error) {
    return {
      schemaVersion: 1,
      kind: "wind-reference-assessment",
      ...(plan.verificationOnly ? { verificationOnly: true } : {}),
      planId: plan.planId,
      candidate: plan.candidate,
      status: "incomplete",
      findings: [error.message],
      captureIdentity: `sha256:${digest(captures)}`,
      captures,
      sourceIdentityStatus: plan.candidate.source?.status ?? "unknown",
      packageGitSha: plan.candidate.source?.packageGitSha ?? null,
      tagToNpmArtifactLink: "unverified",
      acceptedReferenceAdvanced: false,
      reviewedThroughAdvanced: false,
    };
  }
}
