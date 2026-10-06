import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { treeDigest } from "./differential-runner.mjs";
import { digest, fromRoot, sha256 } from "./reference.mjs";

const adapterPaths = [
  "corpus-runner.mjs",
  "corpus-core.mjs",
  "corpus-seeds.mjs",
  "corpus-structure.mjs",
  "corpus-supplemental.mjs",
  "corpus-pilot.mjs",
  "corpus-provenance.mjs",
  "corpus-identity.mjs",
  "browser-adapter.mjs",
  "differential-runner.mjs",
  "differential-core.mjs",
  "spec-identity.mjs",
  "structure.mjs",
  "reference.mjs",
  "reference-module-graph.mjs",
  "oxide-scanner.mjs",
];

export async function currentCorpusIdentity({
  manifest,
  profile,
  contracts,
  pilotObservations,
  executed,
  engine,
  source,
  reference,
  scanner,
  windBuild,
  browserEnvironment,
  rootDir = fromRoot("."),
}) {
  const at = (path) => resolve(rootDir, path);
  const adapterFiles = await Promise.all(
    adapterPaths.map(async (name) => [
      name,
      sha256(await readFile(at(`scripts/wind-compatibility/${name}`))),
    ]),
  );
  return {
    source,
    reference: reference.identity,
    scanner: scanner?.identity ?? null,
    windBuild,
    profileId: profile.profileId,
    profileVersion: profile.profileVersion,
    profileRevision: profile.profileRevision,
    profileDigest: sha256(await readFile(at("tests/wind-compatibility/profile.json"))),
    corpusTreeDigest: await treeDigest(at("tests/wind-compatibility/corpus")),
    pilotFixtureTreeDigest: await treeDigest(at("tests/wind-compatibility/pilot")),
    emptyTokenFixtureTreeDigest: await treeDigest(at("tests/wind-compatibility/empty-token")),
    extractionFixtureTreeDigest: await treeDigest(at("tests/wind-compatibility/extraction")),
    inventoryDigest: sha256(await readFile(at("tests/wind-compatibility/inventory.v1.json"))),
    catalogDigest: sha256(await readFile(at("crates/zudo-wind/catalog/zudo-wind-catalog.v1.json"))),
    sourceInputDigest: digest(
      manifest.upstreamCases.map((row) => [
        row.id,
        row.originalInput,
        row.provenanceAnchors ?? [],
        row.candidates,
      ]),
    ),
    configurationDigest: digest(
      manifest.upstreamCases.map((row) => [
        row.id,
        executed[`upstream/${row.id}/${engine}`]?.configDigest ?? null,
      ]),
    ),
    assertionDigest: digest([
      contracts,
      pilotObservations,
      manifest.compositionObligations,
      manifest.targetedExecutionKeys,
    ]),
    adapterFiles,
    adapterDigest: digest(adapterFiles),
    lockfileDigest: sha256(await readFile(at("pnpm-lock.yaml"))),
    browserEnvironment,
  };
}

export function assertCurrentCorpusIdentity(recorded, expected) {
  if (digest(recorded) !== digest(expected))
    throw Error("Corpus report full current input identity stale or edited");
  return true;
}
