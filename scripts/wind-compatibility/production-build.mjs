import { execFileSync, spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import { mkdir, readFile, realpath, stat, writeFile } from "node:fs/promises";
import { finished } from "node:stream/promises";
import { basename, join, resolve, sep } from "node:path";
import { fromRoot, outsideCheckout, readJson, sha256 } from "./reference.mjs";
import { testedInputIdentity } from "./reference-identity.mjs";

export const productionBuildCommand = "cargo build --locked -p zfb --bin zfb --message-format=json";

export function assertCargoExecution(execution, expected) {
  if (
    execution?.schemaVersion !== 1 ||
    execution.kind !== "zfb-cargo-build-execution" ||
    execution.command !== productionBuildCommand ||
    execution.exitCode !== 0 ||
    execution.sourceShaBefore !== expected.sourceSha ||
    execution.sourceShaAfter !== expected.sourceSha ||
    execution.inputsDigestBefore !== expected.inputsDigest ||
    execution.inputsDigestAfter !== expected.inputsDigest ||
    execution.stdoutPath !== expected.cargoLogPath ||
    execution.stdoutSha256 !== expected.cargoLogSha256 ||
    execution.binaryPath !== expected.binaryPath ||
    execution.binarySha256 !== expected.binarySha256 ||
    execution.stderrSha256 !== expected.stderrSha256 ||
    !execution.startedAt ||
    !execution.finishedAt
  )
    throw Error("Cargo build execution sidecar missing, failed or stale");
}

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

export async function recordProductionBuild(binary, cargoLog, executionPath) {
  const binaryPath = await assertBinary(binary, cargoLog);
  const inputs = await testedInputIdentity();
  const execution = await readJson(executionPath);
  assertCargoExecution(execution, {
    sourceSha: sourceSha(),
    inputsDigest: inputs.digest,
    cargoLogPath: resolve(cargoLog),
    cargoLogSha256: sha256(await readFile(cargoLog)),
    binaryPath,
    binarySha256: sha256(await readFile(binaryPath)),
    stderrSha256: sha256(await readFile(execution.stderrPath)),
  });
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
    cargoExitCode: execution.exitCode,
    executionPath: resolve(executionPath),
    executionSha256: sha256(await readFile(executionPath)),
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
  const execution = await readJson(identity.executionPath);
  assertCargoExecution(execution, {
    sourceSha: identity.gitSha,
    inputsDigest: identity.testedInputsDigest,
    cargoLogPath: identity.cargoLogPath,
    cargoLogSha256: identity.cargoLogSha256,
    binaryPath: identity.binaryPath,
    binarySha256: identity.binarySha256,
    stderrSha256: sha256(await readFile(execution.stderrPath)),
  });
  if (
    binaryPath !== identity.binaryPath ||
    sha256(await readFile(binaryPath)) !== identity.binarySha256 ||
    sha256(await readFile(identity.cargoLogPath)) !== identity.cargoLogSha256 ||
    sha256(await readFile(identity.executionPath)) !== identity.executionSha256 ||
    (await version(binaryPath)) !== identity.version
  )
    throw Error("Production executable or Cargo log changed");
  return identity;
}

export async function runProductionBuild(outputDir) {
  outputDir = await outsideCheckout(outputDir);
  await mkdir(outputDir, { recursive: true });
  const cargoLog = join(outputDir, "cargo.jsonl");
  const stderrPath = join(outputDir, "cargo.stderr.txt");
  const executionPath = join(outputDir, "cargo-execution.json");
  const sourceShaBefore = sourceSha();
  const inputsDigestBefore = (await testedInputIdentity()).digest;
  const startedAt = new Date().toISOString();
  const child = spawn(
    "cargo",
    ["build", "--locked", "-p", "zfb", "--bin", "zfb", "--message-format=json"],
    {
      cwd: fromRoot("."),
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const stdout = createWriteStream(cargoLog);
  const stderr = createWriteStream(stderrPath);
  child.stdout.pipe(stdout);
  child.stderr.pipe(stderr);
  const exitCode = await new Promise((resolveExit, rejectExit) => {
    child.once("error", rejectExit);
    child.once("close", resolveExit);
  });
  await Promise.all([finished(stdout), finished(stderr)]);
  if (exitCode !== 0) throw Error(`Cargo production build exited ${exitCode}; see ${stderrPath}`);
  const sourceShaAfter = sourceSha();
  const inputsDigestAfter = (await testedInputIdentity()).digest;
  if (sourceShaAfter !== sourceShaBefore || inputsDigestAfter !== inputsDigestBefore)
    throw Error("Source or checked inputs changed during Cargo production build");
  const binaryPath = await realpath(executableFromCargoMessages(await readFile(cargoLog)));
  const execution = {
    schemaVersion: 1,
    kind: "zfb-cargo-build-execution",
    command: productionBuildCommand,
    exitCode,
    startedAt,
    finishedAt: new Date().toISOString(),
    sourceShaBefore,
    sourceShaAfter,
    inputsDigestBefore,
    inputsDigestAfter,
    stdoutPath: cargoLog,
    stdoutSha256: sha256(await readFile(cargoLog)),
    stderrPath,
    stderrSha256: sha256(await readFile(stderrPath)),
    binaryPath,
    binarySha256: sha256(await readFile(binaryPath)),
  };
  await writeFile(executionPath, JSON.stringify(execution, null, 2) + "\n");
  const identity = await recordProductionBuild(binaryPath, cargoLog, executionPath);
  const manifestPath = join(outputDir, "production-build.json");
  await writeFile(manifestPath, JSON.stringify(identity, null, 2) + "\n");
  return manifestPath;
}

export async function verifyProductionBuildFile(path, comparison) {
  return verifyProductionBuild(await readJson(path), comparison);
}
