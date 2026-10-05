import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { completeCorpus, expectedObligations, expectedOutcomes, finiteInventoryAccounting, validateCorpus, checkMutations } from "../../../scripts/wind-compatibility/corpus-core.mjs";
import { generatedCandidateLists, generatedSourceStrings, generatedWidths, seed, shrinkFailure, shrinkSourceFailure, shrinkWidthFailure } from "../../../scripts/wind-compatibility/corpus-seeds.mjs";
import { canonical, compareCorpusStructure, expectedWindTree, validateStructureContracts } from "../../../scripts/wind-compatibility/corpus-structure.mjs";
import { parseCssStructure } from "../../../scripts/wind-compatibility/structure.mjs";

const root = new URL("./", import.meta.url);
const json = async (path) => JSON.parse(await readFile(new URL(path, root), "utf8"));
const [manifest, profile, empty, pilot, observations, upstream] = await Promise.all([
  json("manifest.json"), json("../profile.json"), json("../empty-token/manifest.json"),
  json("../pilot/manifest.json"), json("../pilot/observations.json"), json("upstream/manifest.json"),
]);
const probes = Object.fromEntries(await Promise.all(manifest.upstreamCases.map(async (row) => [row.id, await json(`upstream/${row.id}/probes.json`)])));
const contracts = await json("structure-contracts.json");
const copy = (value) => structuredClone(value);
const validate = (m = manifest, p = profile, e = empty, pi = pilot, u = upstream, o = observations, pr = probes) =>
  validateCorpus(m, p, e, pi, u, o, pr);

test("exact corpus membership, source adaptations and composition probes are accounted", () => {
  const expected = validate();
  assert.equal(expected.length, 74);
  assert.equal(expected.filter((id) => id.endsWith("/chromium")).length, 54);
  assert.equal(expected.filter((id) => id.endsWith("/firefox")).length, 10);
  assert.equal(expected.filter((id) => id.endsWith("/webkit")).length, 10);
  assert.equal(manifest.counts.upstreamProbes, 40);
});

test("missing fixtures, duplicate IDs, empty candidate lists and missing execution fail", () => {
  const missing = copy(manifest); missing.upstreamCases.pop();
  assert.throws(() => validate(missing), /membership|counts/);
  const duplicate = copy(manifest); duplicate.upstreamCases[1].id = duplicate.upstreamCases[0].id;
  assert.throws(() => validate(duplicate), /membership|duplicate/);
  const emptyCandidates = copy(manifest); emptyCandidates.upstreamCases[0].candidates = [];
  assert.throws(() => validate(emptyCandidates), /Empty or duplicate candidates/);
  const obligations = expectedObligations(manifest);
  const result = completeCorpus(obligations, Object.fromEntries(obligations.slice(1).map((id) => [id, { outcome: "matched" }])));
  assert.equal(result.complete, false);
  assert.deepEqual(result.missing, [obligations[0]]);
  assert.throws(() => completeCorpus(obligations, { bogus: { outcome: "matched" } }), /Unreviewed/);
  const allowed = expectedOutcomes(manifest, profile);
  const downgraded = Object.fromEntries(obligations.map((id) => [id, { outcome: allowed[id] }]));
  downgraded["upstream/display-flex/chromium"].outcome = "expected-unsupported";
  assert.equal(completeCorpus(obligations, downgraded, allowed).complete, false);
  assert.deepEqual(completeCorpus(obligations, downgraded, allowed).failures, ["upstream/display-flex/chromium"]);
});

test("downgrades and recategorized reviewed differences fail policy validation", () => {
  const downgrade = copy(profile); downgrade.requiredCases[0].disposition = "implementation-gap";
  assert.throws(() => validate(manifest, downgrade), /policy changed/);
  const difference = copy(profile); difference.requiredCases[0].reviewedDifferenceIds = [];
  assert.throws(() => validate(manifest, difference), /policy changed/);
  const reclassified = copy(manifest); reclassified.upstreamCases.find((x) => x.id === "padding-axis").reviewedDifferenceIds = [];
  assert.throws(() => validate(reclassified), /Unreviewed difference/);
  const removedProbe = copy(probes); removedProbe["dark-flex"] = removedProbe["dark-flex"].filter((x) => x.name !== "ancestor");
  assert.throws(() => validate(manifest, profile, empty, pilot, upstream, observations, removedProbe), /probe accounting|Composition reference/);
  const skippedProbe = copy(probes); skippedProbe["hover-flex"].find((x) => x.name === "active").engines = [];
  assert.throws(() => validate(manifest, profile, empty, pilot, upstream, observations, skippedProbe), /observation policy/);
});

test("valid and mutated controls cannot both pass", () => {
  const valid = { "display-valid": true, "hover-valid": true };
  const mutated = { "missing-stylesheet": false, "missing-rule": false, "wrong-declaration": false, "wrong-selector": false, "wrong-media": false };
  assert.equal(checkMutations(valid, mutated).killedMutations, 5);
  assert.throws(() => checkMutations(valid, { ...mutated, "wrong-media": true }), /survived/);
  assert.throws(() => checkMutations({ ...valid, "hover-valid": false }, mutated), /Valid control/);
  assert.throws(() => checkMutations({}, {}), /skipped/);
  const oneMissing = { ...mutated }; delete oneMissing["wrong-selector"];
  assert.throws(() => checkMutations(valid, oneMissing), /skipped/);
});

test("engine and named condition downgrades fail", () => {
  const engineDowngrade = copy(manifest);
  engineDowngrade.upstreamCases.find((row) => row.id === "hover-flex").engines = ["chromium"];
  assert.throws(() => validate(engineDowngrade), /Engine obligation downgraded/);
  const conditionDowngrade = copy(manifest);
  conditionDowngrade.compositionObligations["keyboard-focus"] = ["pilot/block/display"];
  assert.throws(() => validate(conditionDowngrade), /Composition condition membership changed/);
});

test("exact structure detects extra declarations, layers, selector, media and order", () => {
  assert.equal(validateStructureContracts(contracts, manifest, empty), true);
  const alteredContract = copy(contracts);
  alteredContract.cases["display-flex"].rules[0].writes[0][1] = "grid";
  assert.throws(() => validateStructureContracts(alteredContract, manifest, empty), /expectations changed/);
  const emit = (nodes) => nodes.map((node) => node.kind === "statement" ? `${node.text};` : node.kind === "declaration" ? `${node.name}:${node.value};` : `${node.head}{${emit(node.children)}}`).join("");
  const block = contracts.cases["display-flex"];
  const validCss = emit(expectedWindTree(block));
  assert.equal(compareCorpusStructure(block, validCss, "").windPass, true);
  for (const corrupted of [
    validCss.replace("display:flex;", "display:flex;color:red;"),
    validCss.replace(".flex{", "@layer base{.flex{") + "}",
    validCss.replace(".flex{", ".wrong{"),
    validCss.replace("display:flex;", "display:grid;"),
  ]) assert.equal(compareCorpusStructure(block, corrupted, "").windPass, false);
  const hover = contracts.cases["hover-flex"];
  const hoverCss = emit(expectedWindTree(hover));
  assert.equal(compareCorpusStructure(hover, hoverCss.replace("hover: hover", "hover: none"), "").windPass, false);
  const ordered = contracts.cases["flex-direction"];
  const orderedCss = emit(expectedWindTree(ordered));
  assert.equal(compareCorpusStructure(ordered, orderedCss.replace(/(\.flex\{[^}]+\})(\.flex-row\{[^}]+\})/, "$2$1"), "").windPass, false);
  const quoted = (css) => parseCssStructure(css).map(canonical);
  assert.notDeepEqual(quoted(".x{content:'a  b';}"), quoted(".x{content:'a b';}"));
  assert.notDeepEqual(quoted(".x{content:'.5';}"), quoted(".x{content:'0.5';}"));
  assert.notDeepEqual(quoted('.x[data-note="a  b"]{display:block;}'), quoted('.x[data-note="a b"]{display:block;}'));
});

test("finite mapped static domain reports covered and omitted members", async () => {
  const inventory = await json("../inventory.v1.json");
  const catalog = await json("../../../crates/zudo-wind/catalog/zudo-wind-catalog.v1.json");
  const accounting = finiteInventoryAccounting(inventory, catalog, manifest, empty, profile);
  assert.equal(accounting.domainCount, accounting.coveredCount + accounting.omittedCount);
  assert.ok(accounting.coveredRowIds.includes("utility-static:block"));
  assert.ok(accounting.omittedCount > 0);
  assert.equal(accounting.exhaustive, false);
  assert.equal(accounting.keywordDomain.domainCount, accounting.keywordDomain.coveredCount + accounting.keywordDomain.omittedCount);
  assert.ok(accounting.keywordDomain.omittedCount > 0);
});

test("seeded candidate permutations are deterministic and failures shrink", async () => {
  assert.equal(seed, 0x3831c0de);
  const lists = generatedCandidateLists();
  assert.equal(lists.length, 8);
  for (const list of lists) assert.deepEqual([...list].sort(), ["flex", "flex", "flex-row", "relative"].sort());
  const minimized = await shrinkFailure(["relative", "flex", "flex-row", "flex"], async (values) => values.includes("flex-row") && values.includes("flex"));
  assert.deepEqual(minimized, ["flex-row", "flex"]);
  await assert.rejects(() => shrinkFailure(["relative"], async () => false), /passing specimen/);
  const widths = generatedWidths();
  assert.equal(widths.length, 6);
  assert.deepEqual(widths.slice(0, 3), [0, 1, 17]);
  assert.ok(widths.every((value) => Number.isInteger(value) && value >= 0 && value <= 255));
  assert.equal(await shrinkWidthFailure(17, async (value) => value >= 1), 1);
  const sourceStrings = generatedSourceStrings();
  assert.equal(sourceStrings.length, 6);
  assert.ok(sourceStrings.every((value) => value.includes('class="') && value.includes("block") && value.includes("hidden")));
  assert.equal(await shrinkSourceFailure(sourceStrings[0], async () => true), '<span class="block hidden">x</span>');
});
