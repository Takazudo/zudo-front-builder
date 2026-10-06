import { readFile, realpath, writeFile } from "node:fs/promises";
import { isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { treeDigest } from "../../scripts/wind-compatibility/differential-runner.mjs";
import { verifyProductionBuild } from "../../scripts/wind-compatibility/production-build.mjs";
import { testedInputIdentity } from "../../scripts/wind-compatibility/reference-identity.mjs";
import {
  digest,
  fromRoot,
  outsideCheckout,
  sha256,
} from "../../scripts/wind-compatibility/reference.mjs";

export async function verifyRustBuildExport(
  executionPath,
  expectedSha,
  recorded,
  productionBuild,
  distPath,
) {
  executionPath = await outsideCheckout(executionPath);
  const executionBytes = await readFile(executionPath);
  if (sha256(executionBytes) !== expectedSha)
    throw Error("Rust build execution sidecar hash changed");
  const execution = JSON.parse(executionBytes);
  if (digest(execution) !== digest(recorded))
    throw Error("Rust build execution sidecar contents changed");
  const stylesheet = resolve(distPath, execution.stylesheet ?? "");
  const esbuild = execution.buildEnvironment?.ZFB_ESBUILD_BIN;
  if (
    execution.schemaVersion !== 1 ||
    execution.kind !== "zfb-rust-build-export" ||
    execution.binaryPath !== productionBuild.binaryPath ||
    execution.binarySha256 !== productionBuild.binarySha256 ||
    execution.indexSha256 !== sha256(await readFile(resolve(distPath, "index.html"))) ||
    !stylesheet.startsWith(`${distPath}/`) ||
    execution.stylesheetSha256 !== sha256(await readFile(stylesheet)) ||
    execution.buildEnvironment?.PATH !== "" ||
    execution.buildEnvironment?.NODE_PATH !== null ||
    !esbuild ||
    !isAbsolute(esbuild) ||
    execution.buildEnvironment.esbuildSha256 !== sha256(await readFile(esbuild))
  )
    throw Error("Rust build execution does not match current-source dist or Node-free environment");
  return execution;
}

export async function recordDistProof(productionBuildPath, dist, executionPath) {
  const productionBuild = JSON.parse(await readFile(productionBuildPath, "utf8"));
  const inputs = await testedInputIdentity();
  await verifyProductionBuild(productionBuild, {
    testedSourceSha: productionBuild.gitSha,
    testedInputs: inputs,
  });
  const distPath = await realpath(dist);
  const index = await readFile(resolve(distPath, "index.html"));
  if (!index.length) throw Error("Built index.html is empty");
  const execution = JSON.parse(await readFile(executionPath, "utf8"));
  const executionSha256 = sha256(await readFile(executionPath));
  await verifyRustBuildExport(executionPath, executionSha256, execution, productionBuild, distPath);
  return {
    schemaVersion: 1,
    kind: "zfb-current-source-dist",
    productionBuild,
    fixtureTreeDigest: await treeDigest(fromRoot("crates/zfb/tests/fixtures/wind-assets")),
    distPath,
    distTreeDigest: await treeDigest(distPath),
    indexSha256: sha256(index),
    execution,
    executionPath: resolve(executionPath),
    executionSha256,
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
  await verifyRustBuildExport(
    proof.executionPath,
    proof.executionSha256,
    proof.execution,
    proof.productionBuild,
    distPath,
  );
  return proof;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [productionBuild, dist, execution, output] = process.argv.slice(2);
  if (![productionBuild, dist, execution, output].every(Boolean))
    throw Error(
      "Usage: dist-proof.mjs <production-build.json> <dist> <rust-execution-json> <output-json>",
    );
  await writeFile(
    output,
    JSON.stringify(await recordDistProof(productionBuild, dist, execution), null, 2) + "\n",
  );
}
