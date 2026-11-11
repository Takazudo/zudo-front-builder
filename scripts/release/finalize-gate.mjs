#!/usr/bin/env node
// finalize-gate.mjs — admission for release.yml's finalization-only recovery
// (`finalize_only=true`, #4690). Finalization never builds or publishes a
// package; it only reconciles dist-tags, the MD/WASM digest asset, provenance
// expectations and the smoke. It may run only when the publication itself is
// already complete:
//   - every one of the 11 packages is present-verified for the source commit;
//   - every build asset (3 CI archive pairs + the 10 Darwin inputs) is on the
//     Release. A missing build asset is "blocked": use the normal recovery
//     dispatch, which rebuilds and reconciles assets.
//
//   node scripts/release/finalize-gate.mjs --tag vX.Y.Z --source-commit SHA
//        --admission verdict.json --expectations-out FILE [--repo owner/name]
//
// Exit 0 cleared, 1 blocked, 2 usage or read error.
import { readFileSync, writeFileSync } from "node:fs";
import { execGh, isMain, repoFromEnv } from "./gh.mjs";
import { collectStates, expectationShape } from "./npm-package-state.mjs";
import { assetInventory } from "./release-assets-contract.mjs";

/** Pure: the finalization decision. */
export function finalizeDecision({ inventory, npm, admission }) {
  const reasons = [];
  const missingBuild = [...inventory.ci.missing, ...inventory.darwin.missing];
  if (missingBuild.length) {
    reasons.push(
      `blocked: Release build assets missing (${missingBuild.join(", ")}); use the normal recovery dispatch (release_tag without finalize_only)`,
    );
  }
  if (inventory.conflicting.length) {
    reasons.push(
      `blocked: ambiguous or not-uploaded Release assets: ${inventory.conflicting.join(", ")}`,
    );
  }
  if (!admission?.admitted) reasons.push("blocked: Darwin inputs are not admitted");
  for (const state of ["absent", "present-unverified", "uncertain"]) {
    const names = npm.summary[state];
    if (names.length) {
      reasons.push(
        state === "absent"
          ? `blocked: npm packages not published (${names.join(", ")}); use the normal recovery dispatch`
          : `blocked: npm packages ${state} (${names.join(", ")})`,
      );
    }
  }
  return { cleared: reasons.length === 0, reasons };
}

export async function main(argv, { gh = execGh, env = process.env, request } = {}) {
  const opts = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i]?.replace(/^--/, "");
    if (
      !["tag", "source-commit", "admission", "expectations-out", "repo"].includes(key) ||
      !argv[i + 1]
    ) {
      opts.bad = true;
      break;
    }
    opts[key] = argv[i + 1];
  }
  if (
    opts.bad ||
    !/^v\d/.test(opts.tag ?? "") ||
    !/^[0-9a-f]{40}$/.test(opts["source-commit"] ?? "") ||
    !opts.admission ||
    !opts["expectations-out"]
  ) {
    process.stderr.write(
      "usage: finalize-gate.mjs --tag vX.Y.Z --source-commit <40-hex> --admission FILE --expectations-out FILE [--repo owner/name]\n",
    );
    return 2;
  }
  const version = opts.tag.slice(1);
  const repo = opts.repo ?? repoFromEnv(env);
  let admission;
  let assets;
  try {
    admission = JSON.parse(readFileSync(opts.admission, "utf8"));
    assets =
      JSON.parse(gh(["release", "view", opts.tag, "--repo", repo, "--json", "assets"])).assets ??
      [];
  } catch (error) {
    process.stderr.write(`finalize-gate: ${error.message}\n`);
    return 2;
  }
  const npm = await collectStates({
    version,
    sourceCommit: opts["source-commit"],
    verdict: admission,
    ...(request ? { request } : {}),
    ...(env.ZFB_NPM_REGISTRY ? { registry: env.ZFB_NPM_REGISTRY } : {}),
  });
  for (const result of npm.packages) {
    process.stdout.write(
      `  ${result.state.padEnd(19)} ${result.name}@${version}  ${result.error ?? result.detail ?? result.method ?? ""}\n`,
    );
  }
  const decision = finalizeDecision({ inventory: assetInventory(version, assets), npm, admission });
  if (!decision.cleared) {
    for (const reason of decision.reasons) process.stdout.write(`::error::${reason}\n`);
    return 1;
  }
  writeFileSync(
    opts["expectations-out"],
    expectationShape(npm.packages)
      .map(({ name, expected }) => `${name}\t${version}\t${expected}\talready-present\n`)
      .join(""),
  );
  process.stdout.write(
    `Finalization cleared for ${opts.tag}: all 11 packages present-verified, build assets complete.\n`,
  );
  return 0;
}

if (isMain(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
