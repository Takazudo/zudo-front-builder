// Immutable SOURCE data only; never execute source code in the credential job.
import { policyForSource } from "./npm-publication-policy.mjs";
import { isMain } from "./gh.mjs";
export const policyForRelease = (sha, options = {}) =>
  policyForSource(sha, { mode: "recovery-no-provenance", ...options });
if (isMain(import.meta.url)) {
  const policy = policyForRelease(process.argv[2], {
    mode: process.env.ZFB_NPM_PUBLISH_MODE ?? "recovery-no-provenance",
  });
  for (const p of policy.packages) console.log(process.argv.includes("--dirs") ? p.dir : p.name);
}
