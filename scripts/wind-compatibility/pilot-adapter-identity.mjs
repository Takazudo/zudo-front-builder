import { readFile } from "node:fs/promises";
import { digest, fromRoot, sha256 } from "./reference.mjs";

const pilotAdapterPaths = Object.freeze([
  "differential-runner.mjs",
  "browser-adapter.mjs",
  "differential-core.mjs",
  "structure.mjs",
  "oxide-scanner.mjs",
  "reference.mjs",
  "reference-module-graph.mjs",
  "spec-identity.mjs",
  "pilot-adapter-identity.mjs",
]);

export async function pilotAdapterDigest() {
  const hashes = await Promise.all(
    pilotAdapterPaths.map(async (name) =>
      sha256(await readFile(fromRoot(`scripts/wind-compatibility/${name}`))),
    ),
  );
  return digest(hashes);
}

export async function nativeMarginEvidenceDigest() {
  return sha256(
    await readFile(
      fromRoot("tests/wind-compatibility/reference/fixtures/native-margin-37405347552.v1.json"),
    ),
  );
}
