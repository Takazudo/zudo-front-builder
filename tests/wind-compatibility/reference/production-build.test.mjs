import test from "node:test";
import assert from "node:assert/strict";
import {
  executableFromCargoMessages,
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
