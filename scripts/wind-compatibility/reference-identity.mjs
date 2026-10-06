import { execFileSync } from "node:child_process";
import { constants } from "node:fs";
import { lstat, open, readlink } from "node:fs/promises";
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

export async function inputIdentityForPath(path) {
  const stat = await lstat(path);
  // Git records the link itself. Never follow it into an absent directory or
  // outside the checkout; tracked target files have their own identity rows.
  if (stat.isSymbolicLink())
    return { kind: "symlink", sha256: sha256(await readlink(path, { encoding: "buffer" })) };
  if (!stat.isFile()) throw Error(`Unsupported tested input type: ${path}`);
  if (constants.O_NOFOLLOW === undefined)
    throw Error(
      "This platform cannot safely snapshot regular tested inputs without following links",
    );
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const opened = await handle.stat();
    if (!opened.isFile()) throw Error(`Tested input changed to an unsupported type: ${path}`);
    return {
      kind: "file",
      executable: Boolean(opened.mode & 0o111),
      sha256: sha256(await handle.readFile()),
    };
  } finally {
    await handle.close();
  }
}

export async function testedInputIdentity() {
  const paths = testedPaths();
  const files = [];
  // Keep descriptor usage bounded for large checkouts while retaining sorted rows.
  for (let offset = 0; offset < paths.length; offset += 32)
    files.push(
      ...(await Promise.all(
        paths
          .slice(offset, offset + 32)
          .map(async (path) => [path, await inputIdentityForPath(fromRoot(path))]),
      )),
    );
  return { schemaVersion: 2, files, digest: `sha256:${digest(files)}` };
}

export function sameTestedInputs(a, b) {
  return (
    a?.schemaVersion === 2 &&
    b?.schemaVersion === 2 &&
    a.digest === `sha256:${digest(a.files)}` &&
    b.digest === `sha256:${digest(b.files)}` &&
    a.digest === b.digest
  );
}
