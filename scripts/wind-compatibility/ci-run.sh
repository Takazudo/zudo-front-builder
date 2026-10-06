#!/usr/bin/env bash
set -euo pipefail

mode=${1:?expected chromium or full}
evidence=${RUNNER_TEMP:?}/wind-gate
cache=${RUNNER_TEMP}/wind-reference-cache
mkdir -p "$evidence" "$cache"
test -z "$(git status --porcelain)" || { echo 'Dirty source before Wind evidence' >&2; exit 1; }
git rev-parse HEAD > "$evidence/source-sha.txt"
accepted_version=$(node -p 'JSON.parse(require("fs").readFileSync("tests/wind-compatibility/reference/accepted.json")).acceptedReference?.version ?? ""')
if [[ -n $accepted_version ]]; then
  accepted_channel=$(node -p 'JSON.parse(require("fs").readFileSync("tests/wind-compatibility/reference/accepted.json")).acceptedReference.channel')
  node scripts/wind-compatibility/reference-cli.mjs plan --candidate "$accepted_version" --channel "$accepted_channel" --current yes > "$evidence/plan.json"
else
  node scripts/wind-compatibility/reference-cli.mjs plan > "$evidence/plan.json"
fi
candidate_version=$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1])).candidate.version' "$evidence/plan.json")
candidate_channel=$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1])).channel' "$evidence/plan.json")
node scripts/wind-compatibility/reference-cli.mjs acquire --candidate "$candidate_version" --channel "$candidate_channel" --cache "$cache" > "$evidence/acquire.json"
node scripts/wind-compatibility/reference-cli.mjs probe --candidate "$candidate_version" --channel "$candidate_channel" --cache "$cache" --candidates block,hidden,p-0,mx-auto,contents > "$evidence/reference-probe.json"
node scripts/wind-compatibility/reference-cli.mjs assess --plan "$evidence/plan.json" --cache "$cache" > "$evidence/assessment.json"
node --input-type=module -e 'import fs from "node:fs"; const x=JSON.parse(fs.readFileSync(process.argv[1])); if(x.status!=="ready-for-comparison") throw Error("Incomplete reference acquisition")' "$evidence/assessment.json"
node scripts/wind-compatibility/differential-runner.mjs build-wind "$evidence/wind-build.json"
wind_binary=$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1])).binaryPath' "$evidence/wind-build.json")

if [[ $mode == chromium ]]; then
  node --input-type=module -e 'import fs from "node:fs"; const p=JSON.parse(fs.readFileSync(process.argv[1])); const a=JSON.parse(fs.readFileSync(process.argv[2])); const x={...p.candidate, artifactSha256:a.sections.artifacts.tarballSha256}; fs.writeFileSync(process.argv[3], JSON.stringify(x)+"\n")' "$evidence/plan.json" "$evidence/assessment.json" "$evidence/candidate-reference.json"
  common=(--wind-binary "$wind_binary" --wind-build-manifest "$evidence/wind-build.json" --cache "$cache" --reference "$evidence/candidate-reference.json" --engine chromium)
  node scripts/wind-compatibility/differential-runner.mjs "${common[@]}" --output "$evidence/pilot-chromium"
  node scripts/wind-compatibility/corpus-runner.mjs "${common[@]}" --pilot-report "$evidence/pilot-chromium/report.json" --assessment-mode yes --output "$evidence/corpus-chromium"
  node scripts/wind-compatibility/ci-verify.mjs chromium "$evidence" "$cache"
  node scripts/wind-compatibility/ci-gate.mjs produce chromium "$evidence/gate.json" "$evidence/pilot-chromium/report.json" "$evidence/corpus-chromium/report.json"
elif [[ $mode == full ]]; then
  node scripts/wind-compatibility/reference-cli.mjs compare --plan "$evidence/plan.json" --assessment "$evidence/assessment.json" --cache "$cache" --wind-binary "$wind_binary" --wind-build-manifest "$evidence/wind-build.json" --output "$evidence/comparison"
  node --input-type=module -e 'import fs from "node:fs"; const x=JSON.parse(fs.readFileSync(process.argv[1])); if(x.candidate?.passing!==true || x.accepted && x.accepted.passing!==true || x.admission!=="pending-independent-review-and-shipping-evidence") throw Error("Mandatory comparison failed")' "$evidence/comparison/comparison.json"
  # Compile the Rust fixture first. A later `cargo test` invocation can relink
  # target/debug/zfb and invalidate the authenticated production binary hash.
  cargo test --locked -p zfb --test wind_real_build_confirm_build --no-run --message-format=json > "$evidence/fixture-test-cargo.jsonl"
  node --input-type=module -e '
    import fs from "node:fs";
    const artifacts=fs.readFileSync(process.argv[1], "utf8").split("\n").filter(Boolean)
      .map(line=>JSON.parse(line)).filter(x=>x.reason==="compiler-artifact" &&
        x.target?.name==="wind_real_build_confirm_build" && x.target?.kind?.includes("test") &&
        x.profile?.test===true && x.executable);
    if(artifacts.length!==1 || !fs.statSync(artifacts[0].executable).isFile())
      throw Error("Expected exactly one compiled real-build fixture executable");
    const executable=fs.realpathSync(artifacts[0].executable);
    if(!executable.startsWith(fs.realpathSync("target")+"/") ||
       !(fs.statSync(executable).mode & 0o111))
      throw Error("Fixture executable is outside current Cargo target or not executable");
    fs.writeFileSync(process.argv[2], executable+"\n");
  ' "$evidence/fixture-test-cargo.jsonl" "$evidence/fixture-test-binary.txt"
  node scripts/wind-compatibility/build-production.mjs "$evidence/production"
  mkdir -p "$evidence/production/dist"
  fixture_binary=$(cat "$evidence/fixture-test-binary.txt")
  (cd crates/zfb && ZFB_WIND_REAL_BUILD_DIST="$evidence/production/dist" ZFB_WIND_REAL_BUILD_EXECUTION="$evidence/production/build-execution.json" "$fixture_binary" --nocapture)
  node tests/wind-real-build/dist-proof.mjs "$evidence/production/production-build.json" "$evidence/production/dist" "$evidence/production/build-execution.json" "$evidence/production/dist-proof.json"
  node tests/wind-compatibility/shipping/run.mjs --production-build "$evidence/production/production-build.json" --comparison "$evidence/comparison/comparison.json" --plan "$evidence/plan.json" --dist "$evidence/production/dist" --dist-proof "$evidence/production/dist-proof.json" --reference-cache "$cache" --output "$evidence/shipping"
  ZFB_WIND_REAL_BUILD_DIST="$evidence/production/dist" ZFB_WIND_REAL_BUILD_PROOF="$evidence/production/dist-proof.json" node_modules/.bin/playwright test --config tests/wind-real-build/playwright.config.mjs
  node scripts/wind-compatibility/ci-verify.mjs full "$evidence" "$cache"
  node scripts/wind-compatibility/ci-gate.mjs produce full "$evidence/gate.json" "$evidence/comparison/comparison.json" "$evidence/shipping/report.json"
else
  echo "Unknown Wind mode: $mode" >&2; exit 1
fi
test -z "$(git status --porcelain)" || { echo 'Dirty source after Wind evidence' >&2; exit 1; }
DETECTOR_RESULT=success WIND_RELEVANT=true WIND_RESULT=success NATIVE_RESULT=success node scripts/wind-compatibility/ci-gate.mjs verify "$mode" "$evidence/gate.json" "$evidence"
