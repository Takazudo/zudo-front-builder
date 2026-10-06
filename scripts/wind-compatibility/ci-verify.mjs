#!/usr/bin/env node
import { resolve } from "node:path";
import { readJson, identity } from "./reference.mjs";
import { validateRun } from "./reference-comparison.mjs";
import { validateAssessment, validateComparison } from "./reference-promotion.mjs";
import { validateShippingEvidence } from "./reference-shipping.mjs";

const [mode, evidence, cache] = process.argv.slice(2);
if (!["chromium", "full"].includes(mode) || !evidence || !cache)
  throw Error("Usage: ci-verify.mjs chromium|full /outside/evidence /outside/cache");
const plan = await readJson(resolve(evidence, "plan.json"));
const assessment = await readJson(resolve(evidence, "assessment.json"));
const artifactSha256 = await validateAssessment(assessment, plan, cache);
const candidate = { ...plan.candidate, artifactSha256 };
const profile = await readJson("tests/wind-compatibility/profile.json");
const corpus = await readJson("tests/wind-compatibility/corpus/manifest.json");
if (mode === "chromium") {
  const result = await validateRun(evidence, candidate, await identity(), profile, corpus, {
    cache,
    strictArtifacts: true,
    selectedEngines: ["chromium"],
  });
  if (!result.passing) throw Error("Chromium pilot/corpus replay failed");
} else {
  const comparison = await readJson(resolve(evidence, "comparison/comparison.json"));
  const shipping = await readJson(resolve(evidence, "shipping/report.json"));
  await validateComparison({
    comparison,
    plan,
    assessment,
    output: resolve(evidence, "comparison"),
    candidate,
    accepted: plan.previousAccepted,
    input: await identity(),
    cache,
    strictArtifacts: true,
  });
  if (
    comparison.candidate.passing !== true ||
    (comparison.accepted && comparison.accepted.passing !== true)
  )
    throw Error("Full browser matrix has mandatory failures");
  await validateShippingEvidence(
    shipping,
    comparison,
    resolve(evidence, "shipping"),
    plan.toolchain,
  );
}
console.log(`Replayed strict ${mode} Wind evidence`);
