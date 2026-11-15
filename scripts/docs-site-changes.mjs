#!/usr/bin/env node
// Classify a pull request's whole diff against .github/path-filters/docs-site.yml.
// Renames count both names. An unusable rule file or an incomplete file list is
// a full docs check. A failed files API call throws so the detector job fails.
import { appendFileSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { parse } from "yaml";

const PAGE_SIZE = 100;
const PAGE_CAP = 30;

export function compileDocsSitePattern(pattern) {
  let source = "";
  for (let index = 0; index < pattern.length;) {
    if (pattern.startsWith("**/", index)) {
      source += "(?:.+/)?";
      index += 3;
    } else if (pattern.startsWith("**", index)) {
      source += ".*";
      index += 2;
    } else if (pattern[index] === "*") {
      source += "[^/]*";
      index += 1;
    } else if (pattern[index] === "?") {
      source += "[^/]";
      index += 1;
    } else {
      source += pattern[index].replace(/[|\\{}()[\]^$+?.]/g, "\\$&");
      index += 1;
    }
  }
  return new RegExp(`^${source}$`);
}

export function pathMatchesDocsSite(pattern, filePath) {
  return compileDocsSitePattern(pattern).test(filePath);
}

export function readDocsSitePatterns(text) {
  const parsed = parse(text);
  const patterns = parsed?.docs;
  if (
    !Array.isArray(patterns) ||
    patterns.length === 0 ||
    patterns.some((item) => typeof item !== "string" || item === "")
  )
    return null;
  return patterns;
}

export function classifyDocsSiteChanges({ files, patterns, complete }) {
  if (!Array.isArray(patterns) || patterns.length === 0)
    return { docs: true, fallback: "rules-absent", matched: [] };
  if (complete !== true) return { docs: true, fallback: "incomplete", matched: [] };
  if (!Array.isArray(files)) throw new Error("pull request file list is missing");
  const matched = [];
  for (const file of files) {
    for (const filePath of [file?.filename, file?.previous_filename]) {
      if (
        typeof filePath === "string" &&
        patterns.some((pattern) => pathMatchesDocsSite(pattern, filePath))
      )
        matched.push(filePath);
    }
  }
  return { docs: matched.length > 0, fallback: "", matched };
}

export async function listPullRequestFiles({ repo, pr, token, fetchImpl = globalThis.fetch }) {
  if (!repo || !pr || !token)
    throw new Error("pull request file list request is missing repo, pr, or token");
  const files = [];
  for (let page = 1; page <= PAGE_CAP; page += 1) {
    const response = await fetchImpl(
      `https://api.github.com/repos/${repo}/pulls/${pr}/files?per_page=${PAGE_SIZE}&page=${page}`,
      {
        headers: {
          Authorization: `Bearer ${token}`,
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
          "User-Agent": "zfb-docs-site-changes",
        },
      },
    );
    if (!response.ok) throw new Error(`GitHub pull files API failed: ${response.status}`);
    const batch = await response.json();
    if (!Array.isArray(batch)) throw new Error("GitHub pull files API returned a non-array");
    files.push(...batch);
    if (batch.length < PAGE_SIZE) return { files, complete: true };
    if (files.length >= PAGE_SIZE * PAGE_CAP) return { files, complete: false };
  }
  return { files, complete: false };
}

export function formatDetectionOutput(result) {
  return `docs=${result.docs ? "true" : "false"}\ndocs_fallback=${result.fallback}\n`;
}

export async function detectDocsSiteChanges({ rulesText, listFiles }) {
  const patterns = readDocsSitePatterns(rulesText);
  if (patterns === null) return { docs: true, fallback: "rules-unreadable", matched: [] };
  const listed = await listFiles();
  return classifyDocsSiteChanges({ files: listed.files, patterns, complete: listed.complete });
}

async function main() {
  const rulesFlag = process.argv.indexOf("--rules");
  const rulesPath = rulesFlag >= 0 ? process.argv[rulesFlag + 1] : "";
  if (!rulesPath)
    throw new Error("Usage: node scripts/docs-site-changes.mjs --rules <docs-site.yml>");
  const rulesText = readFileSync(rulesPath, "utf8");
  const result = await detectDocsSiteChanges({
    rulesText,
    listFiles: () =>
      listPullRequestFiles({
        repo: process.env.REPO,
        pr: process.env.PR_NUMBER,
        token: process.env.GITHUB_TOKEN,
      }),
  });
  if (!process.env.GITHUB_OUTPUT) throw new Error("GITHUB_OUTPUT is not set");
  appendFileSync(process.env.GITHUB_OUTPUT, formatDetectionOutput(result));
  console.log(
    `docs-site detection: docs=${result.docs} fallback=${result.fallback || "none"} matched=${result.matched.length}`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
