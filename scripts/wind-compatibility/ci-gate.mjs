#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { digest, sha256 } from "./reference.mjs";
import { testedInputIdentity } from "./reference-identity.mjs";
import { requiredMatrixMember } from "./browser-adapter.mjs";

const sha = () => execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
const json = async (path) => JSON.parse(await readFile(path, "utf8"));

export function assertOutcome({
  detector,
  relevant,
  mode,
  native,
  wind,
  evidence,
  sourceSha,
  inputsDigest,
}) {
  if (detector !== "success" || native !== "success")
    throw Error("Required producer/detector failed, cancelled or skipped");
  if (!["true", "false"].includes(relevant) || !["chromium", "full"].includes(mode))
    throw Error("Missing or invalid route");
  if (relevant === "false") {
    if (mode !== "chromium" || wind !== "skipped" || evidence !== null)
      throw Error("Irrelevant PR must be an explicit no-op");
    return "irrelevant-no-op";
  }
  if (wind !== "success" || !evidence)
    throw Error("Required Wind execution failed, cancelled, skipped or missing evidence");
  if (
    evidence.schemaVersion !== 1 ||
    evidence.mode !== mode ||
    evidence.sourceSha !== sourceSha ||
    evidence.testedInputsDigest !== inputsDigest ||
    evidence.result !== "passed" ||
    !Array.isArray(evidence.reports) ||
    evidence.reports.length !== 2 ||
    evidence.reports.some((row) => !/^[0-9a-f]{64}$/.test(row.sha256) || !row.path) ||
    evidence.id !== `sha256:${digest({ ...evidence, id: undefined })}`
  )
    throw Error("Wind evidence missing, stale or malformed");
  const expected =
    mode === "full"
      ? ["comparison/comparison.json", "shipping/report.json"]
      : ["pilot-chromium/report.json", "corpus-chromium/report.json"];
  if (digest(evidence.reports.map((row) => row.path).sort()) !== digest(expected.sort()))
    throw Error("Required evidence membership missing or substituted");
  return "passed";
}

async function produce(mode, output, paths) {
  if (!["chromium", "full"].includes(mode)) throw Error("Invalid evidence mode");
  const profile = await json("tests/wind-compatibility/profile.json");
  const pilotManifest = await json("tests/wind-compatibility/pilot/manifest.json");
  const inputs = await testedInputIdentity();
  const expected =
    mode === "full"
      ? ["comparison/comparison.json", "shipping/report.json"]
      : ["pilot-chromium/report.json", "corpus-chromium/report.json"];
  if (digest(paths.map((path) => path.replace(/^.*\/wind-gate\//, ""))) !== digest(expected))
    throw Error("Wind producer report membership changed");
  const reports = [];
  for (const path of paths) {
    const report = await json(path);
    const { reportId: _id, ...body } = report;
    if (report.reportId !== `sha256:${digest(body)}`) throw Error(`Unsigned report: ${path}`);
    if (mode === "chromium" && path.endsWith("pilot-chromium/report.json")) {
      if (
        report.kind !== "wind-differential-pilot" ||
        report.exitCode !== 0 ||
        report.complete !== true ||
        report.infrastructure ||
        report.browserAdmission ||
        report.identity?.windBuild?.gitSha !== sha() ||
        digest(report.pilotCaseIds) !== digest(pilotManifest.caseIds) ||
        digest(report.cases?.map((row) => row.caseId)) !== digest(pilotManifest.caseIds) ||
        digest(report.controls?.map((row) => row.controlId)) !== digest(profile.requiredControls) ||
        report.cases.some(
          (row) => row.outcome === "unexpected-mismatch" || row.outcome === "missing",
        ) ||
        !requiredMatrixMember(profile, report.identity?.browserEnvironment)
      )
        throw Error("Chromium pilot membership, identity or outcome failed");
    } else if (mode === "chromium") {
      const required = Object.keys(report.executed ?? {}).sort();
      if (
        report.kind !== "wind-independent-corpus" ||
        report.engine !== "chromium" ||
        report.exitCode !== 0 ||
        report.complete !== true ||
        report.infrastructure ||
        report.accounting?.complete !== true ||
        !required.length ||
        digest(required) !== digest([...report.expected].sort()) ||
        report.controls?.mutations?.outcome !== "matched" ||
        report.controls?.seeded?.outcome !== "matched" ||
        !requiredMatrixMember(profile, report.identity?.browserEnvironment) ||
        report.identity?.windBuild?.gitSha !== sha()
      )
        throw Error("Chromium corpus membership, identity or outcome failed");
    } else if (path.endsWith("comparison/comparison.json")) {
      if (
        report.kind !== "wind-three-way-comparison" ||
        report.testedSourceSha !== sha() ||
        report.testedInputs?.digest !== inputs.digest ||
        report.candidate?.passing !== true ||
        (report.accepted && report.accepted.passing !== true) ||
        report.admission !== "pending-independent-review-and-shipping-evidence" ||
        digest(Object.keys(report.candidate?.reports ?? {}).sort()) !==
          digest(
            [
              "pilot-chromium",
              "pilot-firefox",
              "pilot-webkit",
              "corpus-chromium",
              "corpus-firefox",
              "corpus-webkit",
            ].sort(),
          ) ||
        report.browserDelta?.some((row) => !requiredMatrixMember(profile, row.browserEnvironment))
      )
        throw Error("Full comparison identity, matrix or outcome failed");
    } else if (
      report.kind !== "wind-shipping-consumer-evidence" ||
      report.testedSourceSha !== sha() ||
      report.testedInputsDigest !== inputs.digest ||
      !Array.isArray(report.results) ||
      !report.results.length ||
      !report.productionBuild ||
      !report.distProof
    )
      throw Error("Shipping report identity or membership failed");
    reports.push({
      path: path.replace(/^.*\/wind-gate\//, ""),
      sha256: sha256(await readFile(path)),
    });
  }
  const body = {
    schemaVersion: 1,
    mode,
    sourceSha: sha(),
    testedInputsDigest: inputs.digest,
    result: "passed",
    reports,
  };
  const evidence = { ...body, id: `sha256:${digest({ ...body, id: undefined })}` };
  await writeFile(output, `${JSON.stringify(evidence, null, 2)}\n`);
}

async function verify(mode, evidencePath, artifactRoot) {
  const evidence = await json(evidencePath);
  const status = assertOutcome({
    detector: process.env.DETECTOR_RESULT,
    relevant: process.env.WIND_RELEVANT,
    mode,
    native: process.env.NATIVE_RESULT,
    wind: process.env.WIND_RESULT,
    evidence,
    sourceSha: sha(),
    inputsDigest: (await testedInputIdentity()).digest,
  });
  for (const row of evidence.reports) {
    if (row.path.includes("..") || row.path.startsWith("/"))
      throw Error("Artifact path escapes root");
    if (sha256(await readFile(`${artifactRoot}/${row.path}`)) !== row.sha256)
      throw Error(`Artifact changed or missing: ${row.path}`);
  }
  console.log(`Wind required gate: ${status}; ${sha()}`);
}

if (process.argv[1] && import.meta.filename === process.argv[1]) {
  const [command, ...args] = process.argv.slice(2);
  if (command === "produce") await produce(args[0], args[1], args.slice(2));
  else if (command === "verify") await verify(args[0], args[1], args[2]);
  else if (command === "aggregate") {
    const evidence = process.env.WIND_RELEVANT === "true" ? await json(args[0]) : null;
    console.log(
      assertOutcome({
        detector: process.env.DETECTOR_RESULT,
        relevant: process.env.WIND_RELEVANT,
        mode: process.env.WIND_MODE,
        native: process.env.NATIVE_RESULT,
        wind: process.env.WIND_RESULT,
        evidence,
        sourceSha: sha(),
        inputsDigest: (await testedInputIdentity()).digest,
      }),
    );
  } else throw Error("Unknown CI gate command");
}
