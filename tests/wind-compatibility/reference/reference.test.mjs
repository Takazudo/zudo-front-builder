import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gzipSync } from 'node:zlib';
import { acquire, assessBundle, compareVersion, digest, exactVersion, fromRoot, identity, makePlan, probe, requireChannel, resolveCatalog, validatePlan } from '../../../scripts/wind-compatibility/reference.mjs';
import { metadata } from '../../../scripts/wind-compatibility/reference-cli.mjs';

const meta = version => ({ name: 'tailwindcss', version,
  dist: { integrity: 'sha512-AAAAAAAA', tarball: `https://registry.npmjs.org/tailwindcss/-/tailwindcss-${version}.tgz` } });
const catalog = (...items) => ({ complete: true, pages: [{ index: 1, totalPages: 1, truncated: false, next: null, items }] });
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
  assert.throws(() => resolveCatalog({ complete: true, pages: [{ index: 1, totalPages: 1, next: null, items: [meta('4.3.2')] }] }, '4.3.2'), /pagination/);
  assert.throws(() => resolveCatalog({ complete: true, pages: [{ index: 1, totalPages: 1, truncated: false, next: 'page2', items: [meta('4.3.2')] }] }, '4.3.2'), /unvisited/);
  assert.throws(() => resolveCatalog(catalog(meta('5.0.0-next.1')), '5.0.0-next.1'), /Channel/);
  assert.throws(() => requireChannel('5.0.0-next.1', 'stable'), /Channel/);
  assert.throws(() => requireChannel('4.3.2', 'prerelease'), /Channel/);
  const next = resolveCatalog(catalog(meta('5.0.0-next.1')), '5.0.0-next.1', 'prerelease');
  const prior = { package: 'tailwindcss', version: '4.3.2', channel: 'stable', integrity: 'sha512-x', artifactSha256: 'artifact',
    lockfileSha256: 'lock', toolchain: { node: 'test' }, source: { packageGitSha: null, status: 'reviewed-unknown' },
    profileId: 'wind-preset-free', profileVersion: 1, profileRevision: 1, profileDigest: 'profile',
    evidenceReportId: 'report', evidenceReportDigest: 'reportdigest' };
  const transition = makePlan({ candidate: next, state: { ...state, accepted: { schemaVersion: 1, acceptedReference: prior } }, hashes, channel: 'prerelease', toolchain: {} });
  assert.deepEqual(transition.changes, { bootstrap: false, channel: true, major: true, prerelease: true, prereleaseTransition: true });
});

test('plan is immutable and rejects stale profile/reference/lock replay', () => {
  const checked = (value, inputs = hashes) => validatePlan(value, inputs, state, candidate, plan.toolchain);
  assert.equal(checked(plan).planId, plan.planId);
  assert.throws(() => checked({ ...plan, channel: 'prerelease' }), /modified/);
  assert.throws(() => checked(plan, { ...hashes, lock: 'new' }), /Stale/);
  assert.throws(() => checked(plan, { ...hashes, 'scripts/wind-compatibility/upstream.mjs': 'changed' }), /Stale/);
  assert.throws(() => checked(plan, { ...hashes, accepted: 'advanced' }), /Stale/);
  for (const altered of [{ ...plan, schemaVersion: 99 }, { ...plan, changes: { ...plan.changes, major: true } }]) {
    const { planId: _, ...body } = altered;
    assert.throws(() => checked({ ...body, planId: `sha256:${digest(body)}` }), /schema|semantics/);
  }
  const changedCandidate = { ...plan, candidate: { ...candidate, integrity: 'sha512-other' } };
  const { planId: candidateId, ...candidateBody } = changedCandidate;
  assert.throws(() => checked({ ...candidateBody, planId: `sha256:${digest(candidateBody)}` }), /provenance mismatch/);
  const changedToolchain = { ...plan, toolchain: { node: 'v0.0.0' } };
  const { planId: toolchainId, ...toolchainBody } = changedToolchain;
  assert.throws(() => checked({ ...toolchainBody, planId: `sha256:${digest(toolchainBody)}` }), /provenance mismatch/);
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
  assert.equal(assessBundle(bundle, plan).status, 'unverified-capture');
  assert.equal(assessBundle({ ...bundle, tests: { ...bundle.tests, pages: [{ ...bundle.tests.pages[0], truncated: true }] } }, plan).status, 'incomplete');
  assert.equal(assessBundle({ ...bundle, source: { ...bundle.source, sourceDigest: null } }, plan).status, 'incomplete');
  assert.equal(assessBundle({ ...bundle, artifacts: { ...bundle.artifacts, pages: [{ ...bundle.artifacts.pages[0], body: JSON.stringify({ index: 1, totalPages: 1, next: null, totalItems: 1, items: [{ id: 'x' }, { id: 'x' }] }) }] } }, plan).status, 'incomplete');
  assert.equal(assessBundle({ ...bundle, artifacts: { ...bundle.artifacts, pages: [{ ...bundle.artifacts.pages[0], body: JSON.stringify({ index: 1, totalPages: 1, next: null, totalItems: 0, items: [] }) }] } }, plan).status, 'incomplete');
  assert.equal(assessBundle({ ...bundle, planId: 'stale' }, plan).status, 'incomplete');
});

function tarball(version, name = 'tailwindcss') {
  const data = Buffer.from(JSON.stringify({ name, version }));
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
    const wrongName = tarball('4.3.2', 'unrelated-package');
    await assert.rejects(acquire({ ...exact, integrity: `sha512-${createHash('sha512').update(wrongName).digest('base64')}` }, dir, async () => response(200, wrongName)), /identity\/version mismatch/);
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

test('all CLI acquisition entry points enforce exact channel before network access', () => {
  for (const command of ['acquire', 'probe']) {
    const args = [fromRoot('scripts/wind-compatibility/reference-cli.mjs'), command, '--candidate', '5.0.0-next.1', '--cache', '/tmp/wind-channel-test'];
    if (command === 'probe') args.push('--candidates', 'block');
    const run = spawnSync(process.execPath, args, { cwd: fromRoot('.'), encoding: 'utf8' });
    assert.equal(run.status, 1);
    assert.match(run.stderr, /Channel stable does not match exact version/);
  }
});

test('live and catalog bootstrap resolution normalize only matching exact metadata', async () => {
  const bootstrap = JSON.parse(await readFile(fromRoot('tests/wind-compatibility/reference/bootstrap.json'), 'utf8'));
  const registry = { name: 'tailwindcss', version: bootstrap.version,
    dist: { integrity: bootstrap.integrity, shasum: bootstrap.sha1, tarball: bootstrap.tarball },
    repository: { url: bootstrap.source.repository } };
  const dir = await mkdtemp(join(tmpdir(), 'wind-bootstrap-'));
  const catalogPath = join(dir, 'catalog.json');
  const live = async value => ({ ok: true, json: async () => value });
  try {
    await writeFile(catalogPath, JSON.stringify(catalog(registry)));
    const fromLive = await metadata('4.3.2', { live: 'yes' }, bootstrap, async () => live(registry));
    const fromCatalog = await metadata('4.3.2', { catalog: catalogPath }, bootstrap);
    assert.deepEqual(fromLive, fromCatalog);
    const cli = spawnSync(process.execPath, [fromRoot('scripts/wind-compatibility/reference-cli.mjs'), 'plan', '--catalog', catalogPath],
      { cwd: fromRoot('.'), encoding: 'utf8' });
    assert.equal(cli.status, 0, cli.stderr);
    assert.deepEqual(JSON.parse(cli.stdout).candidate, fromCatalog);
    assert.equal(fromLive.artifactSha256, bootstrap.verifiedAcquisition.tarballSha256);
    assert.equal(fromLive.source.observedTagCommit, bootstrap.source.observedTagCommit);
    for (const resolved of [fromLive, fromCatalog]) {
      const replay = makePlan({ candidate: resolved, state, hashes, channel: 'stable', toolchain: {} });
      assert.equal(validatePlan(replay, hashes, state, fromCatalog, {}).planId, replay.planId);
    }
    for (const changed of [
      { ...registry, dist: { ...registry.dist, integrity: 'sha512-wrong' } },
      { ...registry, dist: { ...registry.dist, tarball: 'https://registry.npmjs.org/tailwindcss/-/wrong.tgz' } },
      { ...registry, dist: { ...registry.dist, shasum: '0'.repeat(40) } },
      { ...registry, repository: undefined },
    ]) {
      await assert.rejects(metadata('4.3.2', { live: 'yes' }, bootstrap, async () => live(changed)), /Bootstrap registry\/catalog metadata/);
      await writeFile(catalogPath, JSON.stringify(catalog(changed)));
      await assert.rejects(metadata('4.3.2', { catalog: catalogPath }, bootstrap), /Bootstrap registry\/catalog metadata/);
    }
  } finally { await rm(dir, { recursive: true, force: true }); }
});
