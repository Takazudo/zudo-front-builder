import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fromRoot, sha256 } from "./reference.mjs";

export const archiveName =
  "response-1d73680e19488b19e97ea3c96722e363cc0c4fd118af546857d8703e8f0f9be3";
const sourcePrefix = "tailwindcss-056a1550721d4bf79ff732d5ab9414fa83f7064f/";
const corpusRoot = fromRoot("tests/wind-compatibility/corpus");
const compact = (value) => value.replace(/\s+/g, "");

function testBlock(source, testName, id) {
  const anchor = `test('${testName}'`;
  const start = source.indexOf(anchor);
  if (start < 0) throw Error(`Upstream test absent: ${id}/${testName}`);
  const next = source.indexOf("\ntest('", start + anchor.length);
  return source.slice(start, next < 0 ? undefined : next);
}

export async function verifyUpstreamProvenance(manifest, cache) {
  const archive = resolve(cache, archiveName);
  const bytes = await readFile(archive);
  if (sha256(bytes) !== manifest.sourceArchiveSha256)
    throw Error("Upstream source archive changed");
  const members = new Map();
  const readMember = (path) => {
    if (!path.startsWith("packages/tailwindcss/src/") || path.includes(".."))
      throw Error(`Invalid upstream source path: ${path}`);
    if (!members.has(path))
      members.set(
        path,
        execFileSync("tar", ["-xOzf", archive, sourcePrefix + path], {
          encoding: "utf8",
          maxBuffer: 12 * 1024 * 1024,
        }),
      );
    return members.get(path);
  };
  for (const row of manifest.upstreamCases) {
    const anchors = [
      { kind: "test", path: row.upstreamPath, test: row.upstreamTest, input: row.originalInput },
      ...(row.provenanceAnchors ?? []),
    ];
    for (const anchor of anchors) {
      if (
        !["test", "source"].includes(anchor.kind) ||
        typeof anchor.input !== "string" ||
        !anchor.input ||
        (anchor.kind === "test" && (typeof anchor.test !== "string" || !anchor.test))
      )
        throw Error(`Incomplete upstream provenance anchor: ${row.id}`);
      const source = readMember(anchor.path);
      const block = anchor.kind === "test" ? testBlock(source, anchor.test, row.id) : source;
      if (!compact(block).includes(compact(anchor.input)))
        throw Error(`Upstream original input drift: ${row.id}`);
    }
  }
  const license = execFileSync("tar", ["-xOzf", archive, sourcePrefix + "LICENSE"]);
  if (sha256(license) !== sha256(await readFile(resolve(corpusRoot, "LICENSE.tailwindcss"))))
    throw Error("Upstream license copy changed");
  return {
    sourceArchiveSha256: sha256(bytes),
    sourceTagCommit: manifest.sourceTagCommit,
    packageLink: manifest.sourceToPackageLink,
  };
}
