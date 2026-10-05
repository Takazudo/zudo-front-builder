import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdir, realpath } from 'node:fs/promises';
import { dirname, resolve, sep, basename } from 'node:path';
import { pathToFileURL } from 'node:url';
import { gunzipSync } from 'node:zlib';

export const root = resolve(import.meta.dirname, '../..');
export const paths = {
  profile: 'tests/wind-compatibility/profile.json',
  bootstrap: 'tests/wind-compatibility/reference/bootstrap.json',
  accepted: 'tests/wind-compatibility/reference/accepted.json',
  reviewed: 'tests/wind-compatibility/reference/reviewed-through.json',
  lock: 'pnpm-lock.yaml',
};
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
export function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map(k => [k, stable(value[k])]));
  return value;
}
export const digest = value => sha256(JSON.stringify(stable(value)));
export const readJson = async path => JSON.parse(await readFile(path, 'utf8'));
export const fromRoot = path => resolve(root, path);

export function exactVersion(version) {
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(version);
  if (!match) throw Error(`Expected exact semver, got ${version}`);
  if (match.slice(1, 4).some(x => !Number.isSafeInteger(Number(x))) ||
      match[4]?.split('.').some(x => /^\d+$/.test(x) && x.length > 1 && x.startsWith('0'))) throw Error(`Unsafe or invalid semver ${version}`);
  return { major: Number(match[1]), minor: Number(match[2]), patch: Number(match[3]), prerelease: match[4] ?? null };
}
export function requireChannel(version, channel = 'stable') {
  const prerelease = Boolean(exactVersion(version).prerelease);
  if (!['stable', 'prerelease'].includes(channel) || (channel === 'prerelease') !== prerelease) {
    throw Error(`Channel ${channel} does not match exact version ${version}`);
  }
}
export function compareVersion(a, b) {
  const x = exactVersion(a), y = exactVersion(b);
  for (const key of ['major', 'minor', 'patch']) if (x[key] !== y[key]) return Math.sign(x[key] - y[key]);
  if (x.prerelease === y.prerelease) return 0;
  if (!x.prerelease) return 1;
  if (!y.prerelease) return -1;
  const xs = x.prerelease.split('.'), ys = y.prerelease.split('.');
  for (let i = 0; i < Math.max(xs.length, ys.length); i++) {
    if (xs[i] === undefined) return -1;
    if (ys[i] === undefined) return 1;
    const xn = /^\d+$/.test(xs[i]), yn = /^\d+$/.test(ys[i]);
    if (xn && yn) { const n = BigInt(xs[i]) - BigInt(ys[i]); if (n) return n < 0n ? -1 : 1; }
    else if (xn !== yn) return xn ? -1 : 1;
    else if (xs[i] !== ys[i]) return xs[i] < ys[i] ? -1 : 1;
  }
  return 0;
}
export function validateMetadata(meta, requested) {
  if (meta.name !== 'tailwindcss' || meta.version !== requested) throw Error('Package identity/version mismatch');
  exactVersion(meta.version);
  if (!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(meta.dist?.integrity ?? '') || !/^https:\/\//.test(meta.dist?.tarball ?? '')) throw Error('Missing artifact provenance');
  return { package: meta.name, version: meta.version, integrity: meta.dist.integrity, tarball: meta.dist.tarball,
    sha1: meta.dist.shasum ?? null, source: { repository: meta.repository?.url ?? null, packageGitSha: meta.gitHead ?? null,
      status: meta.gitHead ? 'registry-gitHead-unverified-artifact-link' : 'unknown' } };
}
export function canonicalBootstrapCandidate(bootstrap, resolved) {
  if (resolved.package !== bootstrap.package || resolved.version !== bootstrap.version ||
      resolved.integrity !== bootstrap.integrity || resolved.tarball !== bootstrap.tarball ||
      resolved.sha1 !== bootstrap.sha1 ||
      resolved.source?.repository !== bootstrap.source.repository) {
    throw Error('Bootstrap registry/catalog metadata does not match pinned package, artifact or source identity');
  }
  return { package: bootstrap.package, version: bootstrap.version, integrity: bootstrap.integrity,
    tarball: bootstrap.tarball, sha1: bootstrap.sha1,
    artifactSha256: bootstrap.verifiedAcquisition.tarballSha256, source: bootstrap.source };
}
export function resolveCatalog(catalog, requested, channel = 'stable') {
  if (!['stable', 'prerelease'].includes(channel)) throw Error('Unknown channel');
  if (!Array.isArray(catalog.pages) || !catalog.pages.length || catalog.complete !== true) throw Error('Incomplete catalog');
  const releases = [], seen = new Set();
  for (let i = 0; i < catalog.pages.length; i++) {
    const page = catalog.pages[i];
    if (page.index !== i + 1 || page.totalPages !== catalog.pages.length || page.truncated !== false || !Array.isArray(page.items)) throw Error('Catalog pagination/truncation');
    if (i < catalog.pages.length - 1 && page.next !== catalog.pages[i + 1].url) throw Error('Catalog page cursor mismatch');
    for (const item of page.items) {
      if (seen.has(item.version)) throw Error(`Duplicate release ${item.version}`);
      seen.add(item.version); releases.push(item);
    }
  }
  if (catalog.pages.at(-1).next !== null) throw Error('Catalog has unvisited next page');
  const candidate = releases.find(item => item.version === requested);
  if (!candidate) throw Error(`Release ${requested} absent from complete catalog`);
  requireChannel(requested, channel);
  return validateMetadata(candidate, requested);
}
export async function identity() {
  const result = {};
  for (const [key, path] of Object.entries(paths)) result[key] = sha256(await readFile(fromRoot(path)));
  for (const path of ['scripts/wind-compatibility/reference.mjs', 'scripts/wind-compatibility/reference-cli.mjs', 'scripts/wind-compatibility/upstream.mjs']) result[path] = sha256(await readFile(fromRoot(path)));
  result.runtime = digest({ node: process.version, platform: process.platform, arch: process.arch, executable: await realpath(process.execPath) });
  return result;
}
export function makePlan({ candidate, state, hashes, channel, toolchain }) {
  if (candidate?.package !== 'tailwindcss' || !/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(candidate.integrity ?? '') ||
      candidate.tarball !== `https://registry.npmjs.org/tailwindcss/-/tailwindcss-${candidate.version}.tgz` ||
      (candidate.artifactSha256 !== undefined && !/^[0-9a-f]{64}$/.test(candidate.artifactSha256)) ||
      !candidate.source || !['unknown', 'tag-observed-artifact-link-unverified', 'registry-gitHead-unverified-artifact-link'].includes(candidate.source.status) ||
      (candidate.source.status === 'tag-observed-artifact-link-unverified' &&
        (candidate.source.tag !== `v${candidate.version}` || !/^[0-9a-f]{40}$/.test(candidate.source.observedTagCommit ?? ''))) ||
      (candidate.source.packageGitSha !== null && !/^[0-9a-f]{40}$/.test(candidate.source.packageGitSha ?? ''))) throw Error('Invalid candidate package or provenance');
  if (state.profile.schemaVersion !== 1 || state.profile.profileId !== 'wind-preset-free' || state.profile.profileVersion !== 1 ||
      !Number.isSafeInteger(state.profile.profileRevision) || state.profile.profileRevision < 1 ||
      state.accepted.schemaVersion !== 1 || state.reviewed.schemaVersion !== 1 ||
      state.profile.referencePolicy?.initialState?.acceptedReference !== null || state.profile.referencePolicy?.initialState?.reviewedThrough !== null) {
    throw Error('Unknown profile/reference schema or changed immutable bootstrap policy');
  }
  const accepted = state.accepted.acceptedReference;
  if (accepted !== null && (!accepted || accepted.package !== 'tailwindcss' || !exactVersion(accepted.version) ||
      !['stable', 'prerelease'].includes(accepted.channel) || !accepted.integrity || !accepted.artifactSha256 ||
      !accepted.lockfileSha256 || !accepted.toolchain || !accepted.source || !accepted.profileId ||
      !accepted.profileVersion || !accepted.profileRevision || !accepted.profileDigest ||
      !accepted.evidenceReportId || !accepted.evidenceReportDigest)) throw Error('Malformed accepted reference');
  const reviewed = state.reviewed.reviewedThrough;
  if (reviewed !== null && (!reviewed || reviewed.package !== 'tailwindcss' || !exactVersion(reviewed.version) ||
      !['stable', 'prerelease'].includes(reviewed.channel) || !reviewed.integrity || !reviewed.disposition ||
      !reviewed.planId || !reviewed.assessmentId)) throw Error('Malformed reviewed-through record');
  const parsed = exactVersion(candidate.version);
  requireChannel(candidate.version, channel);
  const prior = state.accepted.acceptedReference;
  if (prior && compareVersion(candidate.version, prior.version) <= 0) throw Error('Candidate must advance accepted reference');
  const plan = {
    schemaVersion: 1, kind: 'wind-reference-plan', channel, candidate, previousAccepted: prior,
    previousReviewedThrough: state.reviewed.reviewedThrough,
    changes: { bootstrap: prior === null, channel: prior ? prior.channel !== channel : false,
      major: prior ? exactVersion(prior.version).major !== parsed.major : false,
      prerelease: Boolean(parsed.prerelease), prereleaseTransition: prior ? Boolean(exactVersion(prior.version).prerelease) !== Boolean(parsed.prerelease) : Boolean(parsed.prerelease) },
    inputs: hashes, toolchain, profile: { id: state.profile.profileId, version: state.profile.profileVersion, revision: state.profile.profileRevision },
    admission: 'unassessed',
  };
  return { ...plan, planId: `sha256:${digest(plan)}` };
}
export function validatePlan(plan, hashes, state, expectedCandidate, expectedToolchain) {
  if (!plan || plan.schemaVersion !== 1 || plan.kind !== 'wind-reference-plan' || plan.admission !== 'unassessed') throw Error('Unknown plan schema or state');
  const { planId, ...body } = plan;
  if (planId !== `sha256:${digest(body)}`) throw Error('Plan has been modified');
  if (JSON.stringify(plan.inputs) !== JSON.stringify(hashes)) throw Error('Stale plan inputs');
  if (digest(plan.candidate) !== digest(expectedCandidate) || digest(plan.toolchain) !== digest(expectedToolchain)) throw Error('Plan candidate/toolchain provenance mismatch');
  const expected = makePlan({ candidate: expectedCandidate, state, hashes, channel: plan.channel, toolchain: expectedToolchain });
  if (digest(plan) !== digest(expected)) throw Error('Plan semantics do not match current state');
  return plan;
}
export function assessBundle(bundle, plan) {
  const missing = [];
  if (!bundle || typeof bundle !== 'object') return { schemaVersion: 1, kind: 'wind-reference-assessment', planId: plan.planId,
    candidate: plan.candidate, status: 'incomplete', missing: ['bundle missing'], bundleDigest: digest(bundle),
    acceptedReferenceAdvanced: false, reviewedThroughAdvanced: false };
  if (bundle.schemaVersion !== 1 || bundle.candidateVersion !== plan.candidate.version || bundle.candidateIntegrity !== plan.candidate.integrity || bundle.planId !== plan.planId) missing.push('bundle identity mismatch');
  if (plan.previousAccepted === null) {
    if (bundle.baselineMode !== 'bootstrap-snapshot' || !bundle.bootstrapRationale || bundle.previousVersion !== null) missing.push('bootstrap snapshot provenance');
  } else if (bundle.baselineMode !== 'release-delta' || bundle.previousVersion !== plan.previousAccepted.version) missing.push('release delta baseline mismatch');
  for (const section of ['changelog', 'artifacts', 'source', 'tests']) {
    const entry = bundle[section];
    if (!entry || entry.complete !== true || !/^https:\/\//.test(entry.source ?? '') || !entry.sourceDigest || !Array.isArray(entry.pages) || !entry.pages.length ||
      !Number.isSafeInteger(entry.totalItems) || entry.totalItems < 0 || !Number.isSafeInteger(entry.totalPages) || entry.totalPages !== entry.pages.length) {
      missing.push(`${section}: missing provenance, totals or pages`); continue;
    }
    if (plan.previousAccepted === null && entry.totalItems === 0) missing.push(`${section}: empty bootstrap snapshot`);
    if (entry.sourceDigest !== `sha256:${digest(entry.pages.map(page => page?.body))}`) missing.push(`${section}: source digest mismatch`);
    const ids = new Set();
    for (let i = 0; i < entry.pages.length; i++) {
      const page = entry.pages[i];
      if (!page || typeof page !== 'object') { missing.push(`${section}: invalid page`); continue; }
      if (page.index !== i + 1 || page.totalPages !== entry.pages.length || page.truncated !== false || !/^https:\/\//.test(page.url ?? '') ||
          page.httpStatus !== 200 || !page.contentType || typeof page.body !== 'string' || !page.body ||
          page.bodySha256 !== sha256(page.body)) { missing.push(`${section}: pagination, fetch or raw body provenance`); continue; }
      if (i < entry.pages.length - 1 && page.next !== entry.pages[i + 1]?.url) missing.push(`${section}: page cursor mismatch`);
      let parsed;
      try { parsed = JSON.parse(page.body); } catch { missing.push(`${section}: invalid raw JSON`); continue; }
      if (parsed.index !== page.index || parsed.totalPages !== page.totalPages || parsed.next !== page.next ||
          parsed.totalItems !== entry.totalItems || !Array.isArray(parsed.items)) { missing.push(`${section}: raw pagination/count mismatch`); continue; }
      for (const item of parsed.items) {
        if (!item.id || ids.has(item.id) || typeof item.content !== 'string' || !item.content.trim() ||
            !/^https:\/\//.test(item.url ?? '') || item.contentSha256 !== sha256(item.content)) missing.push(`${section}: missing/duplicate/unverifiable raw item`);
        ids.add(item.id);
      }
    }
    if (entry.pages.at(-1).next !== null) missing.push(`${section}: unvisited next page`);
    if (ids.size !== entry.totalItems) missing.push(`${section}: item count mismatch`);
  }
  return { schemaVersion: 1, kind: 'wind-reference-assessment', planId: plan.planId, candidate: plan.candidate,
    status: missing.length ? 'incomplete' : 'unverified-capture', missing, bundleDigest: digest(bundle),
    sourceIdentityStatus: plan.candidate.source?.status ?? 'unknown', sourceGitSha: plan.candidate.source?.packageGitSha ?? null,
    acceptedReferenceAdvanced: false, reviewedThroughAdvanced: false };
}

async function physicalPath(path) {
  let current = resolve(path), suffix = [];
  for (;;) {
    try { return resolve(await realpath(current), ...suffix); }
    catch (error) {
      if (error.code !== 'ENOENT') throw error;
      const parent = dirname(current);
      if (parent === current) throw error;
      suffix.unshift(basename(current)); current = parent;
    }
  }
}
export async function outsideCheckout(path) {
  const cache = await physicalPath(path), checkout = await realpath(root);
  if (cache === checkout || cache.startsWith(`${checkout}${sep}`)) throw Error('Cache must be outside checkout');
  return cache;
}

export function tarPackageIdentity(gz) {
  const tar = gunzipSync(gz);
  for (let at = 0; at + 512 <= tar.length;) {
    const name = tar.toString('utf8', at, at + 100).replace(/\0.*$/, '');
    if (!name) break;
    const sizeText = tar.toString('ascii', at + 124, at + 136).replace(/\0.*$/, '').trim();
    const size = parseInt(sizeText, 8);
    if (!Number.isFinite(size)) throw Error('Invalid tar entry size');
    if (name === 'package/package.json') {
      const manifest = JSON.parse(tar.toString('utf8', at + 512, at + 512 + size));
      return { name: manifest.name, version: manifest.version };
    }
    at += 512 + Math.ceil(size / 512) * 512;
  }
  throw Error('package/package.json absent from tarball');
}
export async function probe(acquisition, candidates) {
  if (!Array.isArray(candidates) || !candidates.length || candidates.some(x => typeof x !== 'string' || !x)) throw Error('Probe requires explicit candidates');
  const bytes = await readFile(acquisition.cachePath);
  if (sha256(bytes) !== acquisition.sha256 || JSON.stringify(tarPackageIdentity(bytes)) !== JSON.stringify({ name: 'tailwindcss', version: acquisition.version })) throw Error('Cached artifact identity changed');
  const tar = gunzipSync(bytes), base = await outsideCheckout(resolve(dirname(acquisition.cachePath), `tailwindcss-${acquisition.version}-${acquisition.sha256}`));
  for (let at = 0; at + 512 <= tar.length;) {
    const name = tar.toString('utf8', at, at + 100).replace(/\0.*$/, '');
    if (!name) break;
    const size = parseInt(tar.toString('ascii', at + 124, at + 136).replace(/\0.*$/, '').trim(), 8);
    if (!Number.isFinite(size)) throw Error('Invalid tar entry size');
    if (/^package\/dist\/[A-Za-z0-9_.-]+\.mjs$/.test(name)) {
      const path = resolve(base, name.slice('package/'.length));
      await outsideCheckout(path);
      await mkdir(dirname(path), { recursive: true });
      await outsideCheckout(path);
      const content = tar.subarray(at + 512, at + 512 + size);
      await writeFile(path, content);
    }
    at += 512 + Math.ceil(size / 512) * 512;
  }
  const { compile } = await import(pathToFileURL(resolve(base, 'dist/lib.mjs')).href);
  const compiler = await compile('@theme { --*: initial; } @tailwind utilities;');
  return { schemaVersion: 1, package: acquisition.package, version: acquisition.version,
    integrity: acquisition.integrity, artifactSha256: acquisition.sha256, source: acquisition.source,
    css: compiler.build(candidates), candidates };
}
export async function acquire(meta, cacheDir, fetcher = fetch) {
  if (meta.package !== 'tailwindcss') throw Error('Acquisition package must be tailwindcss');
  const cache = await outsideCheckout(cacheDir);
  const response = await fetcher(meta.tarball);
  if (!response.ok) throw Error(`Artifact fetch HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const [algorithm, expected] = meta.integrity.split('-');
  if (algorithm !== 'sha512' || createHash('sha512').update(bytes).digest('base64') !== expected) throw Error('Artifact integrity mismatch');
  if (meta.sha1 && createHash('sha1').update(bytes).digest('hex') !== meta.sha1) throw Error('Artifact SHA-1 mismatch');
  const embedded = tarPackageIdentity(bytes);
  if (embedded.name !== 'tailwindcss' || embedded.version !== meta.version) throw Error('Tarball package identity/version mismatch');
  await mkdir(cache, { recursive: true });
  await outsideCheckout(cache);
  const output = resolve(cache, `tailwindcss-${meta.version}-${sha256(bytes)}.tgz`);
  await outsideCheckout(output);
  await writeFile(output, bytes, { flag: 'wx' }).catch(async error => { if (error.code !== 'EEXIST' || sha256(await readFile(output)) !== sha256(bytes)) throw error; });
  return { package: meta.package, version: meta.version, integrity: meta.integrity, sha256: sha256(bytes), cachePath: output,
    source: meta.source ?? { packageGitSha: null, status: 'unknown' } };
}
