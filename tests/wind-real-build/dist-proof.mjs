import { readFile, realpath, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { treeDigest } from "../../scripts/wind-compatibility/differential-runner.mjs";
import {
  recordProductionBuild,
  verifyProductionBuild,
} from "../../scripts/wind-compatibility/production-build.mjs";
import { testedInputIdentity } from "../../scripts/wind-compatibility/reference-identity.mjs";
import { fromRoot, sha256 } from "../../scripts/wind-compatibility/reference.mjs";

export async function recordDistProof(binary, cargoLog, dist, executionPath) {
  const productionBuild = await recordProductionBuild(binary, cargoLog);
  const distPath = await realpath(dist);
  const index = await readFile(resolve(distPath, "index.html"));
  if (!index.length) throw Error("Built index.html is empty");
  const execution = JSON.parse(await readFile(executionPath, "utf8"));
  const stylesheet = resolve(distPath, execution.stylesheet ?? "");
  if (
    execution.schemaVersion !== 1 ||
    execution.kind !== "zfb-rust-build-export" ||
    execution.binaryPath !== productionBuild.binaryPath ||
    execution.binarySha256 !== productionBuild.binarySha256 ||
    execution.indexSha256 !== sha256(index) ||
    !stylesheet.startsWith(`${distPath}/`) ||
    execution.stylesheetSha256 !== sha256(await readFile(stylesheet))
  )
    throw Error("Rust build export does not match the current production executable or dist");
  return {
    schemaVersion: 1,
    kind: "zfb-current-source-dist",
    productionBuild,
    fixtureTreeDigest: await treeDigest(fromRoot("crates/zfb/tests/fixtures/wind-assets")),
    distPath,
    distTreeDigest: await treeDigest(distPath),
    indexSha256: sha256(index),
    execution,
  };
}

export async function verifyDistProof(proof, dist) {
  if (proof?.schemaVersion !== 1 || proof.kind !== "zfb-current-source-dist")
    throw Error("Current-source dist proof missing");
  const inputs = await testedInputIdentity();
  await verifyProductionBuild(proof.productionBuild, {
    testedSourceSha: proof.productionBuild.gitSha,
    testedInputs: inputs,
  });
  const distPath = await realpath(dist);
  if (
    proof.distPath !== distPath ||
    proof.fixtureTreeDigest !==
      (await treeDigest(fromRoot("crates/zfb/tests/fixtures/wind-assets"))) ||
    proof.distTreeDigest !== (await treeDigest(distPath)) ||
    proof.indexSha256 !== sha256(await readFile(resolve(distPath, "index.html")))
  )
    throw Error("Built dist bytes, fixture, or path changed");
  const stylesheet = resolve(distPath, proof.execution?.stylesheet ?? "");
  if (
    proof.execution?.kind !== "zfb-rust-build-export" ||
    proof.execution.binaryPath !== proof.productionBuild.binaryPath ||
    proof.execution.binarySha256 !== proof.productionBuild.binarySha256 ||
    proof.execution.indexSha256 !== proof.indexSha256 ||
    !stylesheet.startsWith(`${distPath}/`) ||
    proof.execution.stylesheetSha256 !== sha256(await readFile(stylesheet))
  )
    throw Error("Rust build export proof changed");
  return proof;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [binary, cargoLog, dist, execution, output] = process.argv.slice(2);
  if (![binary, cargoLog, dist, execution, output].every(Boolean))
    throw Error(
      "Usage: dist-proof.mjs <zfb-binary> <cargo-json-log> <dist> <rust-execution-json> <output-json>",
    );
  await writeFile(
    output,
    JSON.stringify(await recordDistProof(binary, cargoLog, dist, execution), null, 2) + "\n",
  );
}
