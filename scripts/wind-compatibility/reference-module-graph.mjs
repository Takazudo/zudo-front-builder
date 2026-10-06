import { lstat, readFile, readdir, realpath } from "node:fs/promises";
import { dirname, relative, resolve, sep } from "node:path";
import { gunzipSync } from "node:zlib";
import { digest, sha256 } from "./reference.mjs";

// The exact .mjs members in the SRI-verified tailwindcss@4.3.2 npm artifact.
export const referenceModuleNames = Object.freeze([
  "chunk-5JIJA4QV.mjs",
  "chunk-HMCCH6MG.mjs",
  "chunk-X4GG3EDV.mjs",
  "colors.mjs",
  "default-theme.mjs",
  "flatten-color-palette.mjs",
  "lib.mjs",
  "plugin.mjs",
]);

export function verifiedReferenceImport(specifier, parentURL, moduleDirectoryURL, names) {
  if (!parentURL?.startsWith(moduleDirectoryURL)) return false;
  if (!/^\.\/[A-Za-z0-9_.-]+\.mjs$/.test(specifier) ||
      !names.has(specifier.slice(2)))
    throw Error(`Unsupported reference module import: ${specifier}`);
  return true;
}

export function archivedReferenceModules(archiveBytes, pinned = false) {
  const tar = gunzipSync(archiveBytes),
    modules = new Map();
  for (let at = 0; at + 512 <= tar.length;) {
    const name = tar.toString("utf8", at, at + 100).replace(/\0.*$/, "");
    if (!name) break;
    const size = parseInt(
      tar
        .toString("ascii", at + 124, at + 136)
        .replace(/\0.*$/, "")
        .trim(),
      8,
    );
    if (!Number.isFinite(size) || size < 0 || at + 512 + size > tar.length)
      throw Error("Invalid reference archive entry");
    if (name.startsWith("/") || name.split("/").includes(".."))
      throw Error(`Unsupported or unsafe reference archive member ${name}`);
    const match = /^package\/dist\/([A-Za-z0-9_.-]+\.mjs)$/.exec(name);
    if (match) {
      const type = tar.toString("ascii", at + 156, at + 157);
      if (!["0", "\0"].includes(type) || modules.has(match[1]))
        throw Error(`Unsafe or duplicate reference module ${match[1]}`);
      modules.set(match[1], Buffer.from(tar.subarray(at + 512, at + 512 + size)));
    }
    at += 512 + Math.ceil(size / 512) * 512;
  }
  if (pinned &&
    JSON.stringify([...modules.keys()].sort()) !== JSON.stringify([...referenceModuleNames].sort())
  )
    throw Error("Pinned reference module set differs from reviewed package");
  if (!modules.has("lib.mjs") || modules.size === 0)
    throw Error("Reference compiler module graph has no entrypoint");
  return modules;
}

export async function verifyExtractedReferenceModules(cacheRoot, modulePath, expectedModules, pinned = false) {
  if (
    dirname(modulePath) === modulePath ||
    modulePath !== resolve(modulePath) ||
    modulePath !== resolve(dirname(modulePath), "lib.mjs")
  )
    throw Error("Unverified compiler entrypoint");
  const physicalCache = await realpath(cacheRoot),
    physicalDir = await realpath(dirname(modulePath));
  if (physicalDir !== physicalCache && !physicalDir.startsWith(`${physicalCache}${sep}`))
    throw Error("Reference module directory escapes cache");
  const rawRelativeDir = relative(resolve(cacheRoot), dirname(modulePath));
  if (
    rawRelativeDir.startsWith("..") ||
    resolve(cacheRoot, rawRelativeDir) !== resolve(dirname(modulePath))
  )
    throw Error("Reference module directory escapes cache");
  let component = resolve(cacheRoot);
  for (const segment of rawRelativeDir.split(sep).filter(Boolean)) {
    component = resolve(component, segment);
    if (!(await lstat(component)).isDirectory())
      throw Error("Reference module directory is a symlink");
  }
  const actualNames = (await readdir(dirname(modulePath)))
    .filter((name) => name.endsWith(".mjs"))
    .sort();
  const expectedNames = [...expectedModules.keys()].sort();
  if (
    JSON.stringify(actualNames) !== JSON.stringify(expectedNames) ||
    (pinned && JSON.stringify(expectedNames) !== JSON.stringify([...referenceModuleNames].sort()))
  )
    throw Error("Extracted reference module set differs from pinned package");
  const moduleDigests = {};
  for (const name of expectedNames) {
    const file = resolve(dirname(modulePath), name);
    if (!(await lstat(file)).isFile() || (await realpath(file)) !== resolve(physicalDir, name))
      throw Error(`Unverified compiler module ${name}`);
    const actual = await readFile(file),
      expected = expectedModules.get(name);
    if (sha256(actual) !== sha256(expected))
      throw Error(`Extracted compiler module ${name} differs from tarball`);
    moduleDigests[name] = sha256(actual);
  }
  return { moduleDigests, moduleGraphSha256: digest(moduleDigests) };
}

export async function verifyPinnedReferenceModuleGraph(
  cacheRoot,
  modulePath,
  archiveBytes,
  expectedSha256,
) {
  if (sha256(archiveBytes) !== expectedSha256)
    throw Error("Pinned reference archive hash mismatch");
  const modules = archivedReferenceModules(archiveBytes, true);
  return verifyExtractedReferenceModules(cacheRoot, modulePath, modules, true);
}
