import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  archiveName,
  verifyUpstreamProvenance,
} from "../../../scripts/wind-compatibility/corpus-provenance.mjs";
import {
  assertWindSpecIdentity,
  currentWindSpecIdentity,
} from "../../../scripts/wind-compatibility/spec-identity.mjs";
import { currentPilotIdentity } from "../../../scripts/wind-compatibility/corpus-pilot.mjs";
import { producedPilotAdapterDigest } from "../../../scripts/wind-compatibility/differential-runner.mjs";
import {
  nativeMarginEvidenceDigest,
  pilotAdapterDigest,
} from "../../../scripts/wind-compatibility/pilot-adapter-identity.mjs";

const root = new URL("../../../", import.meta.url);
const json = async (path) => JSON.parse(await readFile(new URL(path, root), "utf8"));
const cache = process.env.WIND_REFERENCE_CACHE ?? "/tmp/zfb-wind-reference-cache";
const archive = `${cache}/${archiveName}`;

test("pilot adapter binds the exact current profile and Rust spec revision", async () => {
  const profile = await json("tests/wind-compatibility/profile.json");
  const source = await readFile(new URL("crates/zudo-wind/src/lib.rs", root), "utf8");
  assert.equal(assertWindSpecIdentity(profile, source), true);
  assert.deepEqual(currentWindSpecIdentity(profile), { windSpecVersion: 1, windSpecRevision: 14 });
  const pilotManifest = await json("tests/wind-compatibility/pilot/manifest.json");
  const identity = await currentPilotIdentity({
    profile,
    manifest: pilotManifest,
    windBuild: { gitSha: "a".repeat(40) },
    reference: { identity: { version: "4.3.2" } },
    scanner: { identity: { version: "4.3.2" } },
    browserEnvironment: { name: "chromium" },
  });
  assert.equal(identity.windSpecRevision, 14);
  assert.equal(identity.adapterDigest, await pilotAdapterDigest());
  assert.equal(identity.adapterDigest, await producedPilotAdapterDigest());
  assert.equal(identity.nativeMarginEvidenceDigest, await nativeMarginEvidenceDigest());
  assert.throws(
    () =>
      assertWindSpecIdentity(
        profile,
        source.replace("SPEC_REVISION: u32 = 14", "SPEC_REVISION: u32 = 13"),
      ),
    /Wind source spec identity/,
  );
  const stale = structuredClone(profile);
  stale.sourceBaseline.languageSpecRevision = 13;
  assert.throws(() => assertWindSpecIdentity(stale, source), /Wind profile\/spec identity/);
});

test(
  "every upstream original-input anchor replays against the pinned public source archive",
  { skip: !existsSync(archive) && `Pinned source archive unavailable at ${archive}` },
  async () => {
    const manifest = await json("tests/wind-compatibility/corpus/manifest.json");
    assert.equal(manifest.upstreamCases.length, 24);
    const source = await verifyUpstreamProvenance(manifest, cache);
    assert.equal(source.sourceArchiveSha256, manifest.sourceArchiveSha256);
    const forged = structuredClone(manifest);
    forged.upstreamCases.find((row) => row.id === "native-order").originalInput =
      "['order-0', 'order-1', '-order-1']";
    await assert.rejects(
      () => verifyUpstreamProvenance(forged, cache),
      /Upstream original input drift: native-order/,
    );
    const forgedSecondary = structuredClone(manifest);
    forgedSecondary.upstreamCases.find(
      (row) => row.id === "native-svg",
    ).provenanceAnchors[1].input = "'stroke-fictional',";
    await assert.rejects(
      () => verifyUpstreamProvenance(forgedSecondary, cache),
      /Upstream original input drift: native-svg/,
    );
  },
);
