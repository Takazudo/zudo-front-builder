import test from "node:test";
import assert from "node:assert/strict";
import {
  assertCargoExecution,
  executableFromCargoMessages,
  productionBuildCommand,
  verifyProductionBuild,
} from "../../../scripts/wind-compatibility/production-build.mjs";

const artifact = (name, kind, executable) =>
  JSON.stringify({
    reason: "compiler-artifact",
    target: { name, kind },
    executable,
  });

test("Cargo artifact parser accepts one actual zfb binary, not the wind example", () => {
  const example = artifact("wind_fixture_css", ["example"], "/tmp/wind_fixture_css");
  const zfb = artifact("zfb", ["bin"], "/tmp/zfb");
  assert.throws(() => executableFromCargoMessages(Buffer.from(example)), /exactly one zfb/);
  assert.equal(executableFromCargoMessages(Buffer.from(`${example}\n${zfb}\n`)), "/tmp/zfb");
  assert.throws(
    () => executableFromCargoMessages(Buffer.from(`${zfb}\n${zfb}\n`)),
    /exactly one zfb/,
  );
});

test("Cargo success requires an actual matching execution sidecar", () => {
  const expected = {
    sourceSha: "a".repeat(40),
    inputsDigest: "sha256:inputs",
    cargoLogPath: "/tmp/cargo.jsonl",
    cargoLogSha256: "b".repeat(64),
    binaryPath: "/tmp/zfb",
    binarySha256: "c".repeat(64),
    stderrSha256: "d".repeat(64),
  };
  const execution = {
    schemaVersion: 1,
    kind: "zfb-cargo-build-execution",
    command: productionBuildCommand,
    exitCode: 0,
    sourceShaBefore: expected.sourceSha,
    sourceShaAfter: expected.sourceSha,
    inputsDigestBefore: expected.inputsDigest,
    inputsDigestAfter: expected.inputsDigest,
    stdoutPath: expected.cargoLogPath,
    stdoutSha256: expected.cargoLogSha256,
    binaryPath: expected.binaryPath,
    binarySha256: expected.binarySha256,
    stderrSha256: expected.stderrSha256,
    startedAt: "start",
    finishedAt: "end",
  };
  assert.doesNotThrow(() => assertCargoExecution(execution, expected));
  for (const changed of [
    { exitCode: 1 },
    { sourceShaAfter: "e".repeat(40) },
    { stdoutSha256: "0".repeat(64) },
    { binaryPath: "/tmp/wind_fixture_css" },
    { inputsDigestAfter: "sha256:stale" },
  ])
    assert.throws(
      () => assertCargoExecution({ ...execution, ...changed }, expected),
      /sidecar missing, failed or stale/,
    );
});

test("example build identity cannot stand in for the production executable", async () => {
  await assert.rejects(
    verifyProductionBuild(
      { schemaVersion: 1, kind: "wind-example-cargo-artifact" },
      {
        testedSourceSha: "a".repeat(40),
        testedInputs: { digest: "sha256:example" },
      },
    ),
    /Production build source, inputs or Cargo identity invalid/,
  );
});
