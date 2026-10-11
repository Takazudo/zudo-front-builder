// Shared inventory helpers for the saved real-build dist trees: reading a pass
// tree, finding emitted JavaScript, and classifying each file as the islands
// entry, a statically imported shared chunk, or a lazy chunk. Both
// check-budget.mjs and normalize-identity.mjs classify from the same bytes.
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { relative, resolve, sep } from "node:path";

export const javascriptPattern = /\.(?:js|mjs|cjs)$/;
export const entryPattern = /^assets\/islands(?:-[^/]+)?\.js$/;

export function readDirectoryFiles(directory) {
  const files = {};
  const visit = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const absolute = resolve(current, entry.name);
      const stat = lstatSync(absolute);
      if (stat.isSymbolicLink()) {
        throw new Error("artifact tree contains a symbolic link: " + absolute);
      }
      if (entry.isDirectory()) {
        visit(absolute);
      } else if (entry.isFile()) {
        const path = relative(directory, absolute).split(sep).join("/");
        files[path] = readFileSync(absolute);
      } else {
        throw new Error("artifact tree contains a non-file entry: " + absolute);
      }
    }
  };
  visit(directory);
  return files;
}

export function staticImports(code) {
  const paths = [];
  const patterns = [
    /\bimport\s*(?!\()(?:[^;'"()]*?\bfrom\s*)?["'](\.\.?\/[^"']+\.(?:js|mjs|cjs))["']/g,
    /\bexport\s+[^;'"()]*?\bfrom\s*["'](\.\.?\/[^"']+\.(?:js|mjs|cjs))["']/g,
  ];
  for (const pattern of patterns) {
    for (const match of code.matchAll(pattern)) paths.push(match[1]);
  }
  return paths;
}

export function resolveImport(from, specifier) {
  const parts = from.split("/").slice(0, -1).concat(specifier.split("/"));
  const normalized = [];
  for (const part of parts) {
    if (part === "" || part === ".") continue;
    if (part === "..") {
      if (normalized.length === 0) return null;
      normalized.pop();
    } else {
      normalized.push(part);
    }
  }
  return normalized.join("/");
}

export function emittedJavascript(files) {
  const jsPaths = Object.keys(files)
    .filter((path) => javascriptPattern.test(path))
    .sort();
  const entryPaths = jsPaths
    .filter((path) => entryPattern.test(path) && !path.startsWith("assets/islands-chunk-"))
    .sort();
  return { jsPaths, entryPaths };
}

// The initial set is the entry closure over static imports plus any JS linked
// from index.html. Problems are returned, not thrown, so the budget checker
// can keep collecting findings while the normalizer treats them as fatal.
export function initialJavascript(files, jsPaths, entryPaths, label) {
  const initial = new Set();
  const errors = [];
  const html = files["index.html"];
  if (!html) {
    errors.push(label + ": dist is missing index.html");
    return { initial, errors };
  }

  const emitted = new Set(jsPaths);
  const visit = (path) => {
    if (initial.has(path)) return;
    const bytes = files[path];
    if (!bytes) {
      errors.push(label + ": missing imported chunk " + path);
      return;
    }
    initial.add(path);
    const code = bytes.toString("utf8");
    for (const specifier of staticImports(code)) {
      const resolved = resolveImport(path, specifier);
      if (!resolved || !emitted.has(resolved)) {
        errors.push(
          label +
            ": static import does not resolve to an emitted JS file: " +
            path +
            " -> " +
            specifier,
        );
      } else {
        visit(resolved);
      }
    }
  };

  for (const path of entryPaths) visit(path);
  const htmlScripts = [
    ...html
      .toString("utf8")
      .matchAll(/(?:src|href)=["']([^"']+\.(?:js|mjs|cjs))(?:[?#][^"']*)?["']/g),
  ];
  for (const match of htmlScripts) {
    const rawPath = match[1];
    if (/^(?:[a-z]+:)?\/\//i.test(rawPath)) continue;
    const path = rawPath.replace(/^\//, "").split(/[?#]/, 1)[0];
    if (emitted.has(path)) visit(path);
  }
  return { initial, errors };
}

export function chunkRole(file, entryPaths, initial) {
  return entryPaths.includes(file) ? "entry" : initial.has(file) ? "shared" : "lazy";
}
