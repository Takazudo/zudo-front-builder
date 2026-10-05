import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtemp, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gzipSync } from 'node:zlib';
import { acquire, assessBundle, compareVersion, digest, exactVersion, fromRoot, identity, makePlan, probe, resolveCatalog, validatePlan } from '../../../scripts/wind-compatibility/reference.mjs';

const meta = version => ({ name: 'tailwindcss', version,
  dist: { integrity: 'sha512-AAAAAAAA', tarball: `https://registry.npmjs.org/tailwindcss/-/tailwindcss-${version}.tgz` } });
const catalog = (...items) => ({ complete: true, pages: [{ index: 1, totalPages: 1, next: null, items }] });
const hashes = { profile: 'a', bootstrap: 'b', accepted: 'c', reviewed: 'd', lock: 'e' };
const state = { profile: { schemaVersion: 1, profileId: 'wind-preset-free', profileVersion: 1, profileRevision: 1,
  referencePolicy: { initialState: { acceptedReference: null, reviewedThrough: null } } },
  accepted: { schemaVersion: 1, acceptedReference: null }, reviewed: { schemaVersion: 1, reviewedThrough: null } };
const candidate = resolveCatalog(catalog(meta('4.3.2')), '4.3.2');
const plan = makePlan({ candidate, state, hashes, channel: 'stable', toolchain: { node: 'test', lockfileSha256: 'e' } });

test('exact resolution, channels, major transition, duplicates and pagination', () => {
  assert.equal(compareVersion('4.3.2', '4.3.1'), 1);
  assert.equal(compareVersion('5.0.0-next.2', '5.0.0-next.10'), -1);
  assert.throws(() => exactVersion('^4.3.2'));
  for (const invalid of ['4.3.2-next..1', '4.3.2-next.01', '9007199254740992.0.0']) assert.throws(() => exactVersion(invalid));
  assert.throws(() => resolveCatalog(catalog(meta('4.3.2'), meta('4.3.2')), '4.3.2'), /Duplicate/);
  assert.throws(() => resolveCatalog({ ...catalog(meta('4.3.2')), complete: false }, '4.3.2'), /Incomplete/);
  assert.throws(() => resolveCatalog({ complete: true, pages: [{ index: 1, totalPages: 1, next: 'page2', items: [meta('4.3.2')] }] }, '4.3.2'), /unvisited/);
  assert.throws(() => resolveCatalog(catalog(meta('5.0.0-next.1')), '5.0.0-next.1'), /opt-in/);
  const next = resolveCatalog(catalog(meta('5.0.0-next.1')), '5.0.0-next.1', 'prerelease');
  const prior = { package: 'tailwindcss', version: '4.3.2', channel: 'stable', integrity: 'sha512-x', artifactSha256: 'artifact',
    lockfileSha256: 'lock', toolchain: { node: 'test' }, source: { packageGitSha: null, status: 'reviewed-unknown' },
    profileId: 'wind-preset-free', profileVersion: 1, profileRevision: 1, profileDigest: 'profile',
    evidenceReportId: 'report', evidenceReportDigest: 'reportdigest' };
  const transition = makePlan({ candidate: next, state: { ...state, accepted: { schemaVersion: 1, acceptedReference: prior } }, hashes, channel: 'prerelease', toolchain: {} });
  assert.deepEqual(transition.changes, { bootstrap: false, channel: true, major: true, prerelease: true, prereleaseTransition: true });
});

test('plan is immutable and rejects stale profile/reference/lock replay', () => {
  assert.equal(validatePlan(plan, hashes).planId, plan.planId);
  assert.throws(() => validatePlan({ ...plan, channel: 'prerelease' }, hashes), /modified/);
  assert.throws(() => validatePlan(plan, { ...hashes, lock: 'new' }), /Stale/);
  assert.throws(() => validatePlan(plan, { ...hashes, 'scripts/wind-compatibility/reference.mjs': 'changed' }), /Stale/);
  assert.throws(() => validatePlan(plan, { ...hashes, accepted: 'advanced' }), /Stale/);
  assert.throws(() => makePlan({ candidate, state: { ...state, profile: { ...state.profile, schemaVersion: 2 } }, hashes, channel: 'stable', toolchain: {} }), /Unknown/);
});

test('assessment requires every complete delta source and detects truncation, identity and duplicates', () => {
  const section = name => {
    const item = { id: `${name}-1`, url: `https://example.test/${name}/1`, content: `${name} full raw change entry`, contentSha256: createHash('sha256').update(`${name} full raw change entry`).digest('hex') };
    const body = JSON.stringify({ index: 1, totalPages: 1, next: null, totalItems: 1, items: [item] });
    const pages = [{ index: 1, totalPages: 1, truncated: false, next: null, url: `https://example.test/${name}`, httpStatus: 200, contentType: 'application/json', body, bodySha256: createHash('sha256').update(body).digest('hex') }];
    return { complete: true, source: `https://example.test/${name}`, sourceDigest: `sha256:${digest(pages.map(page => page.body))}`, totalItems: 1, totalPages: 1, pages };
  };
  const bundle = { schemaVersion: 1, candidateVersion: candidate.version, candidateIntegrity: candidate.integrity, planId: plan.planId,
    baselineMode: 'bootstrap-snapshot', previousVersion: null, bootstrapRationale: 'First exact candidate; no historical consumer baseline',
    changelog: section('changelog'), artifacts: section('artifacts'), source: section('source'), tests: section('tests') };
  assert.equal(assessBundle(bundle, plan).status, 'ready-for-comparison');
  assert.equal(assessBundle({ ...bundle, tests: { ...bundle.tests, pages: [{ ...bundle.tests.pages[0], truncated: true }] } }, plan).status, 'incomplete');
  assert.equal(assessBundle({ ...bundle, source: { ...bundle.source, sourceDigest: null } }, plan).status, 'incomplete');
  assert.equal(assessBundle({ ...bundle, artifacts: { ...bundle.artifacts, pages: [{ ...bundle.artifacts.pages[0], body: JSON.stringify({ index: 1, totalPages: 1, next: null, totalItems: 1, items: [{ id: 'x' }, { id: 'x' }] }) }] } }, plan).status, 'incomplete');
  assert.equal(assessBundle({ ...bundle, artifacts: { ...bundle.artifacts, pages: [{ ...bundle.artifacts.pages[0], body: JSON.stringify({ index: 1, totalPages: 1, next: null, totalItems: 0, items: [] }) }] } }, plan).status, 'incomplete');
  assert.equal(assessBundle({ ...bundle, planId: 'stale' }, plan).status, 'incomplete');
});

function tarball(version) {
  const data = Buffer.from(JSON.stringify({ name: 'tailwindcss', version }));
  const header = Buffer.alloc(512);
  header.write('package/package.json'); header.write(data.length.toString(8).padStart(11, '0'), 124);
  const pad = Buffer.alloc(Math.ceil(data.length / 512) * 512 - data.length);
  return gzipSync(Buffer.concat([header, data, pad, Buffer.alloc(1024)]));
}
test('acquisition rejects offline, auth, 404, integrity mismatch and wrong package version', async () => {
  const checkoutStatus = () => execFileSync('git', ['status', '--porcelain=v1', '--untracked-files=all'], { cwd: fromRoot('.') }).toString();
  const before = { status: checkoutStatus(), inputs: await identity() };
  const dir = await mkdtemp(join(tmpdir(), 'wind-ref-test-'));
  const bytes = tarball('4.3.2');
  const exact = { ...candidate, integrity: `sha512-${createHash('sha512').update(bytes).digest('base64')}` };
  const response = (status, body = bytes) => ({ ok: status === 200, status, arrayBuffer: async () => body });
  try {
    await assert.rejects(acquire(exact, dir, async () => { throw Error('offline'); }), /offline/);
    await assert.rejects(acquire(exact, dir, async () => response(401)), /401/);
    await assert.rejects(acquire(exact, dir, async () => response(404)), /404/);
    await assert.rejects(acquire(candidate, dir, async () => response(200)), /integrity mismatch/);
    const wrong = tarball('4.3.3');
    await assert.rejects(acquire({ ...exact, integrity: `sha512-${createHash('sha512').update(wrong).digest('base64')}` }, dir, async () => response(200, wrong)), /version mismatch/);
    assert.equal((await acquire(exact, dir, async () => response(200))).version, '4.3.2');
    await assert.rejects(acquire(exact, fromRoot('tests/wind-compatibility'), async () => response(200)), /outside checkout/);
    const link = join(dir, 'checkout-link');
    await symlink(fromRoot('.'), link);
    await assert.rejects(acquire(exact, join(link, 'tests', 'wind-compatibility', 'cache'), async () => response(200)), /outside checkout/);
    const artifactPath = join(dir, 'artifact.tgz'), artifactSha = createHash('sha256').update(bytes).digest('hex');
    await writeFile(artifactPath, bytes);
    await symlink(fromRoot('.'), join(dir, `tailwindcss-4.3.2-${artifactSha}`));
    await assert.rejects(probe({ package: 'tailwindcss', version: '4.3.2', sha256: artifactSha, cachePath: artifactPath, integrity: exact.integrity }, ['block']), /outside checkout/);
  } finally { await rm(dir, { recursive: true, force: true }); }
  assert.deepEqual({ status: checkoutStatus(), inputs: await identity() }, before);
});

test('CLI dry run leaves checkout tracked and untracked identity unchanged', async () => {
  const status = () => execFileSync('git', ['status', '--porcelain=v1', '--untracked-files=all'], { cwd: fromRoot('.') }).toString();
  const before = { status: status(), inputs: await identity() };
  const run = spawnSync(process.execPath, [fromRoot('scripts/wind-compatibility/reference-cli.mjs'), 'plan'], { cwd: fromRoot('.'), encoding: 'utf8' });
  assert.equal(run.status, 0, run.stderr);
  const emitted = JSON.parse(run.stdout);
  assert.equal(emitted.previousAccepted, null);
  assert.equal(emitted.previousReviewedThrough, null);
  assert.deepEqual({ status: status(), inputs: await identity() }, before);
});
