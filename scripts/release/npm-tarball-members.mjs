import { posix } from "node:path";
import { gunzipSync } from "node:zlib";
import { parseTarEntries } from "../admit-darwin-artifacts.mjs";

export function indexNpmPayload(entries) {
  const files = new Map();
  const directories = new Set();
  for (const entry of entries) {
    const { name, typeflag } = entry;
    const directory = typeflag === "5";
    const destination = directory ? name.replace(/\/$/, "") : name;
    if (directory && destination === "package") continue;
    if (
      !["0", "5"].includes(typeflag) ||
      !destination.startsWith("package/") ||
      destination.endsWith("/") ||
      destination.includes("\\") ||
      destination.includes("\0") ||
      posix.normalize(destination) !== destination
    ) {
      throw new Error(
        `noncanonical npm tarball path or unsupported entry type: ${JSON.stringify(name)}`,
      );
    }
    const path = destination.slice("package/".length);
    if (files.has(path)) {
      const conflict = directory ? "file/directory conflict" : "duplicate file destination";
      throw new Error(`npm tarball ${conflict}: ${destination}`);
    }
    if (directory) {
      directories.add(path);
    } else {
      if (directories.has(path)) {
        throw new Error(`npm tarball file/directory conflict: ${destination}`);
      }
      files.set(path, entry);
    }
    let parent = posix.dirname(path);
    while (parent !== ".") {
      if (files.has(parent)) {
        throw new Error(`npm tarball file/directory conflict: package/${parent}`);
      }
      directories.add(parent);
      parent = posix.dirname(parent);
    }
  }
  return files;
}

export function readNpmPayload(bytes, { maxOutputLength, onGunzipError } = {}) {
  let tar;
  try {
    tar = gunzipSync(bytes, { maxOutputLength });
  } catch (error) {
    if (onGunzipError) onGunzipError(error);
    throw error;
  }
  return indexNpmPayload(parseTarEntries(tar, { strictPax: true }));
}
