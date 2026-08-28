import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

import {
  REQUIRED_CONSUMER_ARTIFACTS,
  findMissingConsumerArtifacts,
  formatMissingArtifactsError,
} from "../../crates/zfb-md-wasm/npm/scripts/assert-consumer-artifacts.mjs";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const packageJson = JSON.parse(readFileSync(join(rootDir, "package.json"), "utf8"));
const mdWasmPackageJson = JSON.parse(
  readFileSync(join(rootDir, "crates/zfb-md-wasm/npm/package.json"), "utf8"),
);
const healthWorkflow = readFileSync(join(rootDir, ".github/workflows/health.yml"), "utf8");
const b4push = readFileSync(join(rootDir, "scripts/run-b4push.sh"), "utf8");
const temporaryDirectories = [];

function workflowJob(workflow, name) {
  const start = workflow.indexOf(`  ${name}:\n`);
  if (start === -1) throw new Error(`missing workflow job: ${name}`);

  const remainder = workflow.slice(start + 1);
  const next = remainder.search(/^  [a-zA-Z0-9_-]+:\n/m);
  return workflow.slice(start, next === -1 ? undefined : start + 1 + next);
}

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("workspace test contract", () => {
  it("defines canonical ordinary, typecheck, and md-wasm lanes", () => {
    expect(packageJson.scripts["test:workspace"]).toBe(
      "pnpm -r --include-workspace-root --filter '!@takazudo/zfb-md-wasm' test",
    );
    expect(packageJson.scripts["typecheck:workspace"]).toBe(
      "pnpm -r --filter '!./examples/*' --filter '!@takazudo/zfb-md-wasm' --if-present typecheck",
    );
    expect(packageJson.scripts["test:md-wasm"]).toBe(
      "pnpm --filter @takazudo/zfb-md-wasm build && pnpm --filter @takazudo/zfb-md-wasm test",
    );
    expect(mdWasmPackageJson.scripts.pretest).toBe("node scripts/assert-consumer-artifacts.mjs");
    expect(mdWasmPackageJson.scripts["typecheck:consumer"]).toBe(
      "tsc --project test/tsconfig.consumer.json",
    );
  });

  it("keeps health and b4push on the canonical workspace scripts", () => {
    expect(healthWorkflow).toContain("- run: pnpm typecheck:workspace");
    expect(healthWorkflow).toContain("- run: pnpm test:workspace");
    expect(b4push).toContain("if pnpm typecheck:workspace; then");
    expect(b4push).toContain("if pnpm test:workspace; then");

    expect(healthWorkflow).not.toContain("pnpm -r --filter '!./examples/*'");
    expect(healthWorkflow).not.toContain("pnpm -r --include-workspace-root");
    expect(b4push).not.toContain("pnpm -r --filter '!./examples/*'");
    expect(b4push).not.toContain("pnpm -r --include-workspace-root");
  });

  it("keeps the wasm-md artifact build before its package tests", () => {
    const wasmMdJob = workflowJob(healthWorkflow, "wasm-md");
    const buildStep = wasmMdJob.indexOf("node scripts/run-zfb-md-wasm-build-timed.mjs");
    const testStep = wasmMdJob.indexOf("run: pnpm --filter @takazudo/zfb-md-wasm test");

    expect(wasmMdJob).toContain("name: Build four @takazudo/zfb-md-wasm artifacts (timed)");
    expect(buildStep).toBeGreaterThanOrEqual(0);
    expect(testStep).toBeGreaterThanOrEqual(0);
    expect(buildStep).toBeLessThan(testStep);
  });

  it("requires every consumer declaration artifact, not only the dist directory", () => {
    const packageRoot = mkdtempSync(join(tmpdir(), "zfb-md-wasm-consumer-guard-"));
    temporaryDirectories.push(packageRoot);
    mkdirSync(join(packageRoot, "dist"));

    expect(findMissingConsumerArtifacts(packageRoot)).toEqual(REQUIRED_CONSUMER_ARTIFACTS);

    writeFileSync(join(packageRoot, "dist", "index.js"), "");
    expect(findMissingConsumerArtifacts(packageRoot)).not.toContain("dist/index.js");
    expect(findMissingConsumerArtifacts(packageRoot)).toContain("dist/index.d.ts");

    mkdirSync(join(packageRoot, "dist", "render.js"));
    expect(findMissingConsumerArtifacts(packageRoot)).toContain("dist/render.js");
    rmSync(join(packageRoot, "dist", "render.js"), { recursive: true });

    for (const relativePath of REQUIRED_CONSUMER_ARTIFACTS) {
      writeFileSync(join(packageRoot, relativePath), "");
    }
    expect(findMissingConsumerArtifacts(packageRoot)).toEqual([]);
    expect(formatMissingArtifactsError(["dist/index.js"])).toContain(
      "pnpm --filter @takazudo/zfb-md-wasm build",
    );
    expect(formatMissingArtifactsError(["dist/index.js"])).toContain("pnpm test:workspace");
  });
});
