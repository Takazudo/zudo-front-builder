import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const readJson = (name) => JSON.parse(readFileSync(new URL(name, import.meta.url), 'utf8'));
const manifest = readJson('./manifest.json');
const configurations = readJson('./configurations.json');
const profile = readJson('../profile.json');
const html = readFileSync(new URL('./index.html', import.meta.url), 'utf8');
const cases = new Map(manifest.cases.map((entry) => [entry.caseId, entry]));

const sameMembers = (actual, expected) => {
  assert.deepEqual([...actual].sort(), [...expected].sort());
};

test('canonical profile IDs stay fixed while supplemental cases are selectable', () => {
  assert.deepEqual(manifest.profileCaseIds, profile.requiredCases.map((entry) => entry.id));
  assert.deepEqual(
    manifest.cases.slice(0, manifest.profileCaseIds.length).map((entry) => entry.caseId),
    manifest.profileCaseIds,
  );
  assert.deepEqual(
    manifest.cases.slice(manifest.profileCaseIds.length).map((entry) => entry.caseId),
    manifest.supplementalCaseIds,
  );
  assert.equal(cases.size, manifest.cases.length, 'case IDs must be unique');
  sameMembers(manifest.configurationIds, Object.keys(configurations).filter((key) => key !== 'schemaVersion'));
  for (const id of manifest.configurationIds) {
    assert.ok(manifest.cases.some((entry) => entry.configuration === id), `${id} has no selectable case`);
    assert.ok(configurations[id].referenceTheme, `${id} lacks an explicit reference theme`);
  }
});

test('every selectable case has source elements and every observation resolves', () => {
  for (const entry of manifest.cases) {
    assert.ok(configurations[entry.configuration], `${entry.caseId}: unknown configuration`);
    assert.ok(html.includes(`data-case-id="${entry.caseId}"`), `${entry.caseId}: no HTML section`);
    assert.ok(entry.explicitCandidates.length > 0, `${entry.caseId}: no candidate`);
  }
  for (const observation of manifest.browserObservations) {
    if (observation.caseId) assert.ok(cases.has(observation.caseId), `${observation.id}: unknown case`);
    for (const id of [observation.target, observation.control]) {
      assert.ok(html.includes(`id="${id}"`), `${observation.id}: missing ${id}`);
    }
  }
});

test('changed, removed, and palette opt-in paths have complete expected contracts', () => {
  for (const id of manifest.supplementalCaseIds) {
    const entry = cases.get(id);
    assert.ok(entry.expectedWind, `${id}: missing Wind expectation`);
    for (const key of ['browserTargets', 'browserControls', 'browserExpected']) {
      sameMembers(Object.keys(entry[key]), entry.explicitCandidates);
    }
    for (const candidate of entry.explicitCandidates) {
      const target = entry.browserTargets[candidate];
      const control = entry.browserControls[candidate];
      assert.ok(html.includes(`id="${target}"`), `${id}: missing target ${candidate}`);
      assert.ok(html.includes(`id="${control}"`), `${id}: missing control ${candidate}`);
    }
  }
  sameMembers(
    manifest.transitions.map((entry) => entry.to),
    ['semantic-changed-composed', 'numeric-spacing-removed', 'palette-gray-500-opt-in'],
  );
  for (const transition of manifest.transitions) {
    assert.ok(cases.has(transition.from), `${transition.id}: missing previous case`);
    assert.ok(cases.has(transition.to), `${transition.id}: missing next case`);
    assert.equal(cases.get(transition.to).transitionFrom, transition.from);
    assert.notDeepEqual(
      configurations[cases.get(transition.from).configuration].tokens,
      configurations[cases.get(transition.to).configuration].tokens,
      `${transition.id}: transition does not change tokens`,
    );
  }
  assert.deepEqual(cases.get('numeric-spacing-removed').expectedWind, {
    utilityWrites: {},
    tokenDeclarations: {},
    stylesheet: 'empty',
    preludeOccurrences: 0,
    diagnostic: {
      code: 'ZW006',
      rejectionId: 'R17',
      sourceClassSeverity: 'error',
      sourceLiteralSeverity: 'audit-info',
      manifestSeverity: 'error',
    },
  });
  assert.deepEqual(cases.get('palette-gray-500-opt-in').expectedWind.tokenDeclarations, {
    '--zw-color-gray-500': '#37628a',
  });
  assert.deepEqual(cases.get('semantic-changed-composed').expectedWind.tokenDeclarations, {
    '--zw-spacing-hsp-sm': '23px',
    '--zw-spacing-vsp-md': '31px',
    '--zw-color-surface': '#654321',
  });
});
