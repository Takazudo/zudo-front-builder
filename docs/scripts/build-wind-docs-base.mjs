#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

export function configAtBase(source, base) {
  if (!/^\/(?:[a-z0-9-]+\/)+$/.test(base)) {
    throw new Error("A non-root slash-delimited base path is required");
  }
  const declaration = /^    base: "\/",$/gm;
  if ([...source.matchAll(declaration)].length !== 1) {
    throw new Error("Expected exactly one literal root base declaration in docs config");
  }
  return source.replace(declaration, `    base: ${JSON.stringify(base)},`);
}

export function buildAtBase(
  base,
  { configPath = resolve(root, "docs/zfb.config.ts"), run = spawnSync } = {},
) {
  const original = readFileSync(configPath, "utf8");
  const configured = configAtBase(original, base);
  try {
    writeFileSync(configPath, configured);
    const result = run("pnpm", ["--filter", "docs", "build"], { cwd: root, stdio: "inherit" });
    if (result.error) throw result.error;
    if (result.signal || result.status !== 0) {
      throw new Error(`Subpath docs build failed: ${result.signal ?? result.status}`);
    }
  } finally {
    writeFileSync(configPath, original);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 3)
      throw new Error("Usage: node docs/scripts/build-wind-docs-base.mjs /prefix/");
    buildAtBase(process.argv[2]);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
