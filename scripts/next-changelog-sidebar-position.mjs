#!/usr/bin/env node

import { readdirSync, readFileSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const RELEASE_PAGE_NAME = /^v.*\.mdx$/;
const FRONTMATTER = /^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/;
const SIDEBAR_POSITION = /^sidebar_position\s*:\s*(.*?)\s*$/;

function invalidPosition(filePath, detail) {
  return new Error(`${filePath}: invalid sidebar_position (${detail})`);
}

/**
 * Read and validate a release page's sidebar position.
 *
 * Positions are deliberately constrained to positive integers. A fractional,
 * zero, negative, or otherwise malformed value could make a release appear to
 * succeed while silently corrupting a lane's ordering, so the helper fails
 * closed instead of guessing.
 */
export function readSidebarPosition(filePath) {
  let text;
  try {
    text = readFileSync(filePath, "utf8");
  } catch (error) {
    throw new Error(`${filePath}: cannot read changelog page (${error.message})`);
  }

  const frontmatter = text.match(FRONTMATTER);
  if (!frontmatter) {
    throw invalidPosition(filePath, "missing YAML frontmatter");
  }

  const matches = frontmatter[1]
    .split(/\r?\n/)
    .map((line) => line.match(SIDEBAR_POSITION))
    .filter(Boolean);

  if (matches.length === 0) {
    throw invalidPosition(filePath, "missing field");
  }
  if (matches.length > 1) {
    throw invalidPosition(filePath, "duplicate field");
  }

  const rawValue = matches[0][1];
  if (!/^\d+$/.test(rawValue)) {
    throw invalidPosition(filePath, `expected a positive integer, got ${JSON.stringify(rawValue)}`);
  }

  const position = Number(rawValue);
  if (!Number.isSafeInteger(position) || position <= 0) {
    throw invalidPosition(filePath, `expected a positive integer, got ${JSON.stringify(rawValue)}`);
  }
  return position;
}

/**
 * Compute the next position for a target page without letting that target
 * influence the result when it was pre-authored before the release command.
 */
export function nextChangelogSidebarPosition(targetPagePath) {
  if (typeof targetPagePath !== "string" || targetPagePath.trim() === "") {
    throw new Error("target page path is required");
  }

  const targetPath = resolve(targetPagePath);
  const laneDirectory = dirname(targetPath);
  const targetName = basename(targetPath);

  let entries;
  try {
    entries = readdirSync(laneDirectory, { withFileTypes: true });
  } catch (error) {
    throw new Error(`${laneDirectory}: cannot read changelog lane (${error.message})`);
  }

  let maximum = 0;
  for (const entry of entries) {
    if (entry.name === targetName || !RELEASE_PAGE_NAME.test(entry.name)) continue;

    const siblingPath = resolve(laneDirectory, entry.name);
    const position = readSidebarPosition(siblingPath);
    maximum = Math.max(maximum, position);
  }

  return maximum + 1;
}

function main() {
  const args = process.argv.slice(2);
  if (args.length !== 1) {
    throw new Error("usage: next-changelog-sidebar-position.mjs <target-page-path>");
  }

  process.stdout.write(`${nextChangelogSidebarPosition(args[0])}\n`);
}

const invokedPath = process.argv[1] && resolve(process.argv[1]);
const thisPath = fileURLToPath(import.meta.url);
if (invokedPath === thisPath) {
  try {
    main();
  } catch (error) {
    console.error(`next-changelog-sidebar-position: ${error.message}`);
    process.exitCode = 1;
  }
}
