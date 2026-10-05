import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { archiveSources, extractSurface, validateInventory, loadJson, digest } from '../../scripts/wind-compatibility/inventory.mjs';
const root = new URL('../../', import.meta.url);
const at = (path) => new URL(path, root);
const [upstream, inventory, catalog, profile] = await Promise.all([
  loadJson(at('tests/wind-compatibility/upstream-surface.v1.json')),
  loadJson(at('tests/wind-compatibility/inventory.v1.json')),
  loadJson(at('crates/zudo-wind/catalog/zudo-wind-catalog.v1.json')),
  loadJson(at('tests/wind-compatibility/profile.json')),
]);
const runtime = await loadJson(at('tests/wind-compatibility/runtime-keysets.v1.json'));
const variantSource = await readFile(at('crates/zudo-wind/src/variant.rs'), 'utf8');
const docsEvidence = await Promise.all(['docs/wind-examples/guide-coming-from-tailwind.json', 'docs/wind-examples/guide-utility-grammar.json'].map(async (path) => ({ path, sha256: digest(await readFile(at(path))) })));
const copy = () => structuredClone(inventory);

test('pinned membership, evidence and profile cases validate', () => {
  const result = validateInventory(inventory, upstream, catalog, profile, variantSource, docsEvidence, runtime);
  assert.equal(result.rows, 1288);
  assert.equal(inventory.wind.specRevision, 12);
  assert.equal(inventory.wind.catalogEntryCount, 197);
  assert.equal(inventory.profileCases.length, profile.requiredCases.length);
  assert.equal(inventory.profileCases.find((row) => row.id === 'profile:mx-auto').disposition, 'intentional-difference');
  assert.equal(inventory.profileCases.find((row) => row.id === 'profile:p-0-empty').disposition, 'intentional-difference');
  assert(inventory.rows.some((row) => row.disposition === 'excluded-deferred'));
  assert(inventory.rows.some((row) => row.disposition === 'native-css-review-candidate'));
  assert(inventory.rows.some((row) => row.disposition === 'unmapped-upstream-registration'));
});

test('an added upstream literal member fails instead of disappearing', () => {
  const changed = structuredClone(upstream);
  changed.staticUtilities.push('fabricated-new-utility');
  assert.throws(() => validateInventory(inventory, changed, catalog, profile, variantSource, docsEvidence, runtime), /Unclassified upstream source member/);
});

test('an added runtime static or functional member fails unclassified', () => {
  const changed = structuredClone(runtime);
  changed.staticUtilities.push('fabricated-runtime-static');
  assert.throws(() => validateInventory(inventory, upstream, catalog, profile, variantSource, docsEvidence, changed), /Runtime keyset drift/);
  const changedFunctional = structuredClone(runtime);
  changedFunctional.functionalUtilities.push('fabricated-runtime-family');
  assert.throws(() => validateInventory(inventory, upstream, catalog, profile, variantSource, docsEvidence, changedFunctional), /Runtime keyset drift/);
});

test('dynamic source sites also fail when added without review', () => {
  const changed = structuredClone(upstream);
  changed.dynamicSourceSites.push({ id: 'utilities.ts:9999:new', file: 'utilities.ts', line: 9999, expression: 'newName' });
  assert.throws(() => validateInventory(inventory, changed, catalog, profile, variantSource, docsEvidence, runtime), /Unaccounted dynamic/);
});

test('a new template registration is not hidden by literal extraction', () => {
  const files = { 'utilities.ts': `staticUtility('block', [])\nstaticUtility(\`new-${'${name}'}\`, [])`,
    'variants.ts': "variants.static('hover', () => {})",
    'utilities.test.ts': 'test', 'variants.test.ts': 'test',
    'compat/legacy-utilities.ts': 'legacy', 'compat/legacy-utilities.test.ts': 'test' };
  const extracted = extractSurface(files);
  assert(extracted.dynamicSourceSites.some((site) => site.expression.includes('new-${name}')));
  assert.throws(() => validateInventory(inventory, extracted, catalog, profile, variantSource, docsEvidence, runtime));
});

test('duplicate IDs, missing mappings, stale catalog and stale profile case fail', () => {
  const duplicate = copy(); duplicate.rows[1].id = duplicate.rows[0].id;
  assert.throws(() => validateInventory(duplicate, upstream, catalog, profile, variantSource, docsEvidence, runtime));
  const missing = copy();
  const mapped = missing.rows.find((row) => row.windMapping);
  mapped.windMapping = null;
  assert.throws(() => validateInventory(missing, upstream, catalog, profile, variantSource, docsEvidence, runtime), /mapping/);
  const stale = copy(); stale.wind.catalogEntryCount--;
  assert.throws(() => validateInventory(stale, upstream, catalog, profile, variantSource, docsEvidence, runtime), /Stale Wind catalog/);
  const lost = copy(); lost.profileCases.pop();
  assert.throws(() => validateInventory(lost, upstream, catalog, profile, variantSource, docsEvidence, runtime), /Stale profile cases/);
});

test('reverse Wind membership and content pins catch same-count changes', () => {
  assert.equal(inventory.windEntries.length, 197);
  const removed = copy(); removed.windEntries.pop();
  assert.throws(() => validateInventory(removed, upstream, catalog, profile, variantSource, docsEvidence, runtime), /Unaccounted Wind/);
  const changedCatalog = structuredClone(catalog); changedCatalog.entries[0].root = 'fabricated';
  assert.throws(() => validateInventory(inventory, upstream, changedCatalog, profile, variantSource, docsEvidence, runtime), /content drift/);
  const changedProfile = structuredClone(profile); changedProfile.requiredCases[0].expectedContract = 'changed';
  assert.throws(() => validateInventory(inventory, upstream, catalog, changedProfile, variantSource, docsEvidence, runtime), /content drift/);
  const changedDocs = structuredClone(docsEvidence); changedDocs[0].sha256 = '0'.repeat(64);
  assert.throws(() => validateInventory(inventory, upstream, catalog, profile, variantSource, changedDocs, runtime), /docs cross-check/);
  const wrongMapping = copy();
  const exact = wrongMapping.rows.find((r) => r.windMapping?.kind === 'exact-catalog');
  exact.windMapping.catalogId = catalog.entries.find((e) => e.id !== exact.windMapping.catalogId).id;
  assert.throws(() => validateInventory(wrongMapping, upstream, catalog, profile, variantSource, docsEvidence, runtime));
});

test('every runtime static keyword intersection has its exact Wind catalog mapping', () => {
  let intersections = 0;
  for (const row of inventory.rows.filter((item) => item.upstream.kind === 'utility-static'))
    for (const entry of catalog.entries)
      for (const value of entry.acceptedValues ?? [])
        if (value.kind === 'keyword' && row.upstream.name === `${entry.root}-${value.suffix}`) {
          intersections++;
          assert.equal(row.windMapping?.catalogId, entry.id, row.id);
          assert.equal(row.implementation, 'source-inspected-exact-registration', row.id);
        }
  assert.equal(intersections, 238);
  for (const name of ['mx-auto', 'my-auto', 'm-auto', 'cursor-pointer', 'h-full', '-m-px'])
    assert.equal(inventory.rows.find((row) => row.id === `utility-static:${name}`).windMapping?.kind, 'exact-catalog');
});

test('related logical and physical axis cases remain explicit reviewed differences', () => {
  for (const name of ['mx-auto', 'my-auto', 'mx-px', 'my-px', 'px-px', 'py-px', 'inset-x-auto', 'scroll-mx-px']) {
    const row = inventory.rows.find((item) => item.id === `utility-static:${name}`);
    assert.match(row.semanticDifference, /logical axes.*physical axes/);
    assert.equal(row.disposition, 'reviewed-difference');
  }
  for (const name of ['mx', 'my', 'px', 'py', '-mx', '-my', '-inset-x', '-scroll-mx']) {
    const row = inventory.rows.find((item) => item.id === `utility-pattern:${name}`);
    assert.match(row.semanticDifference, /logical axes.*physical axes/);
    assert.equal(row.disposition, 'requires-review');
  }
});

test('forged evidence, semantic differences, mappings and pattern claims fail canonical replay', () => {
  const mutate = (change) => { const value = copy(); change(value); assert.throws(() => validateInventory(value, upstream, catalog, profile, variantSource, docsEvidence, runtime), /reviewed source-derived contract/); };
  mutate((value) => { const row = value.profileCases.find((item) => item.id === 'profile:block'); row.evidence.status = 'independently-differential-tested'; row.evidence.reportId = 'fabricated'; row.evidence.browserEnvironment = 'fabricated'; });
  mutate((value) => { value.profileCases.find((item) => item.id === 'profile:contents-gap').gapKind = null; });
  mutate((value) => { value.rows.find((item) => item.id === 'utility-static:mx-auto').semanticDifference = null; });
  mutate((value) => { value.rows.find((item) => item.id === 'variant-static:hover').windMapping.wind = 'disabled'; });
  mutate((value) => { const row = value.configuredPatterns.find((item) => item.id === 'pattern:arbitrary'); row.wind = '<any-arbitrary>'; row.disposition = 'supported'; });
  mutate((value) => { value.windEntries[0].evidence.reportId = 'fabricated'; });
});

test('profile evidence never claims an executed report or browser', () => {
  for (const row of [...inventory.rows, ...inventory.profileCases]) {
    assert.equal(row.evidence.status, 'source-inspected');
    assert.equal(row.evidence.reportId, null);
    assert.equal(row.evidence.browserEnvironment, null);
  }
});

if (process.env.WIND_PINNED_SOURCE_ARCHIVE) test('checked snapshot matches independently captured pinned archive', async () => {
  const files = archiveSources(await readFile(process.env.WIND_PINNED_SOURCE_ARCHIVE));
  assert.deepEqual(extractSurface(files), upstream);
});

if (process.env.WIND_NPM_TARBALL && process.env.WIND_COMPILER_MODULE) test('tampered imported compiler chunk is rejected before runtime load', async () => {
  const { mkdtemp, readdir, copyFile, writeFile, rm } = await import('node:fs/promises');
  const { dirname, join } = await import('node:path');
  const { tmpdir } = await import('node:os');
  const { verifyRuntime } = await import('../../scripts/wind-compatibility/inventory.mjs');
  const directory = await mkdtemp(join(tmpdir(), 'wind-inventory-module-'));
  try {
    const sourceDir = dirname(process.env.WIND_COMPILER_MODULE);
    const modules = (await readdir(sourceDir)).filter((name) => name.endsWith('.mjs'));
    for (const name of modules) await copyFile(join(sourceDir, name), join(directory, name));
    const chunk = modules.find((name) => name.startsWith('chunk-'));
    assert(chunk);
    await writeFile(join(directory, chunk), 'tampered');
    await assert.rejects(verifyRuntime(runtime, process.env.WIND_NPM_TARBALL, join(directory, 'lib.mjs')), /differs from tarball/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

if (process.env.WIND_NPM_TARBALL && process.env.WIND_COMPILER_MODULE) test('unverified extra compiler entrypoint is rejected', async () => {
  const { mkdtemp, readdir, copyFile, writeFile, rm } = await import('node:fs/promises');
  const { dirname, join } = await import('node:path');
  const { tmpdir } = await import('node:os');
  const { verifyRuntime } = await import('../../scripts/wind-compatibility/inventory.mjs');
  const directory = await mkdtemp(join(tmpdir(), 'wind-inventory-entry-'));
  try {
    for (const name of (await readdir(dirname(process.env.WIND_COMPILER_MODULE))).filter((item) => item.endsWith('.mjs')))
      await copyFile(join(dirname(process.env.WIND_COMPILER_MODULE), name), join(directory, name));
    await writeFile(join(directory, 'evil.mjs'), 'export const fake = true;');
    await assert.rejects(verifyRuntime(runtime, process.env.WIND_NPM_TARBALL, join(directory, 'evil.mjs')), /Unverified compiler entrypoint/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

if (process.env.WIND_NPM_TARBALL && process.env.WIND_COMPILER_MODULE) test('symlinked lib entrypoint cannot redirect imported chunks', async () => {
  const { mkdtemp, readdir, copyFile, symlink, rm } = await import('node:fs/promises');
  const { dirname, join } = await import('node:path');
  const { tmpdir } = await import('node:os');
  const { verifyRuntime } = await import('../../scripts/wind-compatibility/inventory.mjs');
  const directory = await mkdtemp(join(tmpdir(), 'wind-inventory-symlink-'));
  try {
    const safe = join(directory, 'safe');
    const alternate = join(directory, 'alternate');
    const { mkdir } = await import('node:fs/promises');
    await mkdir(safe); await mkdir(alternate);
    for (const name of (await readdir(dirname(process.env.WIND_COMPILER_MODULE))).filter((item) => item.endsWith('.mjs')))
      await copyFile(join(dirname(process.env.WIND_COMPILER_MODULE), name), join(safe, name));
    await copyFile(process.env.WIND_COMPILER_MODULE, join(alternate, 'lib.mjs'));
    await rm(join(safe, 'lib.mjs'));
    await symlink(join(alternate, 'lib.mjs'), join(safe, 'lib.mjs'));
    await assert.rejects(verifyRuntime(runtime, process.env.WIND_NPM_TARBALL, join(safe, 'lib.mjs')), /Unverified compiler entrypoint/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
