import { execFileSync } from "node:child_process";
import { readFile, realpath, stat } from "node:fs/promises";
import { basename, resolve, sep } from "node:path";
import { fromRoot, readJson, sha256 } from "./reference.mjs";
import { testedInputIdentity } from "./reference-identity.mjs";

export const productionBuildCommand = "cargo build --locked -p zfb --bin zfb --message-format=json";

export function executableFromCargoMessages(bytes) {
  const artifacts = bytes
    .toString("utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .filter(
      (message) =>
        message.reason === "compiler-artifact" &&
        message.target?.name === "zfb" &&
        message.target?.kind?.includes("bin") &&
        message.executable,
    );
  if (artifacts.length !== 1) throw Error("Cargo log must contain exactly one zfb binary artifact");
  return artifacts[0].executable;
}

function sourceSha() {
  return execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: fromRoot("."),
    encoding: "utf8",
  }).trim();
}

async function assertBinary(binary, cargoLog) {
  const artifact = executableFromCargoMessages(await readFile(cargoLog));
  const actual = await realpath(binary);
  if (actual !== (await realpath(artifact)) || basename(actual) !== "zfb")
    throw Error("Production executable is not the zfb Cargo artifact");
  const root = await realpath(fromRoot("."));
  if (!actual.startsWith(root + sep))
    throw Error("Production executable must belong to the current source checkout");
  if (!(await stat(actual)).isFile()) throw Error("Production executable is not a file");
  return actual;
}

async function version(binary) {
  const { NODE_PATH: _nodePath, ...environment } = process.env;
  const value = execFileSync(binary, ["--version"], {
    encoding: "utf8",
    env: { ...environment, PATH: "" },
  }).trim();
  if (!/^zfb\s+\S+/.test(value)) throw Error("Production executable did not identify as zfb");
  return value;
}

export async function recordProductionBuild(binary, cargoLog) {
  const binaryPath = await assertBinary(binary, cargoLog);
  const inputs = await testedInputIdentity();
  return {
    schemaVersion: 1,
    kind: "zfb-production-cargo-artifact",
    gitSha: sourceSha(),
    testedInputsDigest: inputs.digest,
    binaryPath,
    binarySha256: sha256(await readFile(binaryPath)),
    version: await version(binaryPath),
    cargoLogPath: resolve(cargoLog),
    cargoLogSha256: sha256(await readFile(cargoLog)),
    cargoCommand: productionBuildCommand,
    cargoExitCode: 0,
    lockDigest: sha256(await readFile(fromRoot("Cargo.lock"))),
  };
}

export async function verifyProductionBuild(identity, comparison) {
  if (
    identity?.schemaVersion !== 1 ||
    identity.kind !== "zfb-production-cargo-artifact" ||
    identity.gitSha !== sourceSha() ||
    identity.gitSha !== comparison.testedSourceSha ||
    identity.testedInputsDigest !== comparison.testedInputs.digest ||
    identity.cargoCommand !== productionBuildCommand ||
    identity.cargoExitCode !== 0 ||
    identity.lockDigest !== sha256(await readFile(fromRoot("Cargo.lock"))) ||
    !/^[0-9a-f]{64}$/.test(identity.binarySha256 ?? "") ||
    !/^[0-9a-f]{64}$/.test(identity.cargoLogSha256 ?? "")
  )
    throw Error("Production build source, inputs or Cargo identity invalid");
  const binaryPath = await assertBinary(identity.binaryPath, identity.cargoLogPath);
  if (
    binaryPath !== identity.binaryPath ||
    sha256(await readFile(binaryPath)) !== identity.binarySha256 ||
    sha256(await readFile(identity.cargoLogPath)) !== identity.cargoLogSha256 ||
    (await version(binaryPath)) !== identity.version
  )
    throw Error("Production executable or Cargo log changed");
  return identity;
}

export async function verifyProductionBuildFile(path, comparison) {
  return verifyProductionBuild(await readJson(path), comparison);
}
