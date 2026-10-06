import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fromRoot, digest, sha256 } from "./reference.mjs";

// Only the two transition records are excluded from their own preimage. This
// deliberately includes tracked and unignored untracked Markdown/MDX fixtures, preview pages, policy prose,
// runner contracts, lockfiles and production sources. Reports live outside the
// checkout and are not tracked inputs.
export function testedPaths() {
  const listed = execFileSync(
    "git",
    ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
    { cwd: fromRoot(".") },
  )
    .toString("utf8")
    .split("\0")
    .filter(Boolean);
  return listed
    .filter(
      (path) =>
        ![
          "tests/wind-compatibility/reference/accepted.json",
          "tests/wind-compatibility/reference/reviewed-through.json",
        ].includes(path),
    )
    .sort();
}

export async function testedInputIdentity() {
  const files = await Promise.all(
    testedPaths().map(async (path) => [path, sha256(await readFile(fromRoot(path)))]),
  );
  return { schemaVersion: 1, files, digest: `sha256:${digest(files)}` };
}

export function sameTestedInputs(a, b) {
  return (
    a?.schemaVersion === 1 &&
    b?.schemaVersion === 1 &&
    a.digest === `sha256:${digest(a.files)}` &&
    b.digest === `sha256:${digest(b.files)}` &&
    a.digest === b.digest
  );
}
