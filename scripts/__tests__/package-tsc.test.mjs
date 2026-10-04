// Tests for scripts/package-tsc.mjs (#3556): package-local compiler discovery that keeps working
// when TypeScript 7's `exports` map hides `./bin/tsc`.

import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vite-plus/test";

import { resolvePackageTsc } from "../package-tsc.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const roots = [];

function makeConsumer() {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "package-tsc-")));
  roots.push(root);
  const consumer = join(root, "consumer");
  mkdirSync(consumer, { recursive: true });
  writeFileSync(join(consumer, "package.json"), JSON.stringify({ name: "consumer" }));
  return consumer;
}

function installCompiler(consumer, directoryName, manifest, binSource) {
  const packageRoot = join(consumer, "node_modules", directoryName);
  mkdirSync(join(packageRoot, "bin"), { recursive: true });
  writeFileSync(join(packageRoot, "package.json"), JSON.stringify(manifest));
  const binPath = join(packageRoot, "bin", "tsc");
  writeFileSync(binPath, binSource);
  chmodSync(binPath, 0o755);
  return binPath;
}

const NODE_BIN = '#!/usr/bin/env node\nconsole.log("tsc " + process.argv.slice(2).join(" "));\n';

afterEach(() => {
  while (roots.length > 0) rmSync(roots.pop(), { recursive: true, force: true });
});

describe("resolvePackageTsc", () => {
  it("launches a TypeScript 7-shaped package whose exports map hides ./bin/tsc", () => {
    const consumer = makeConsumer();
    const binPath = installCompiler(
      consumer,
      "typescript",
      {
        name: "typescript",
        version: "7.0.2",
        type: "module",
        bin: { tsc: "./bin/tsc" },
        exports: { "./package.json": "./package.json", ".": "./lib/version.cjs" },
      },
      NODE_BIN,
    );
    const require = createRequire(join(consumer, "package.json"));
    expect(() => require.resolve("typescript/bin/tsc")).toThrow(
      /ERR_PACKAGE_PATH_NOT_EXPORTED|not defined by "exports"/,
    );

    const tsc = resolvePackageTsc(consumer);
    expect(tsc).toEqual({
      version: "7.0.2",
      binPath,
      command: process.execPath,
      args: [binPath],
    });
    const run = spawnSync(tsc.command, [...tsc.args, "--version"], {
      cwd: tmpdir(),
      encoding: "utf8",
    });
    expect(run.status).toBe(0);
    expect(run.stdout).toBe("tsc --version\n");
  });

  it("launches a TypeScript 6-shaped package without an exports map", () => {
    const consumer = makeConsumer();
    const binPath = installCompiler(
      consumer,
      "typescript",
      { name: "typescript", version: "6.0.3", bin: { tsc: "./bin/tsc" } },
      NODE_BIN,
    );
    expect(resolvePackageTsc(consumer)).toMatchObject({ version: "6.0.3", args: [binPath] });
  });

  it("runs a non-node bin directly instead of through node", () => {
    const consumer = makeConsumer();
    const binPath = installCompiler(
      consumer,
      "typescript",
      { name: "typescript", version: "9.9.9", bin: "./bin/tsc" },
      "#!/bin/sh\necho native\n",
    );
    expect(resolvePackageTsc(consumer)).toMatchObject({ command: binPath, args: [] });
  });

  it("resolves an npm alias of the compiler for consumer-floor probes", () => {
    const consumer = makeConsumer();
    installCompiler(
      consumer,
      "typescript",
      { name: "typescript", version: "6.0.3", bin: { tsc: "./bin/tsc" } },
      NODE_BIN,
    );
    const aliasBin = installCompiler(
      consumer,
      "typescript-5.9",
      { name: "typescript", version: "5.9.3", bin: { tsc: "./bin/tsc" } },
      NODE_BIN,
    );
    expect(resolvePackageTsc(consumer, "typescript-5.9")).toMatchObject({
      version: "5.9.3",
      args: [aliasBin],
    });
  });

  it("rejects an alias that is not the typescript compiler", () => {
    const consumer = makeConsumer();
    installCompiler(
      consumer,
      "typescript-5.9",
      { name: "not-typescript", version: "1.0.0", bin: { tsc: "./bin/tsc" } },
      NODE_BIN,
    );
    expect(() => resolvePackageTsc(consumer, "typescript-5.9")).toThrow(
      /not the typescript compiler/,
    );
  });

  it("fails when the package has no compiler instead of falling back to another one", () => {
    const consumer = makeConsumer();
    expect(() => resolvePackageTsc(consumer)).toThrow(/cannot resolve typescript\/package\.json/);
  });

  it("finds each workspace package's pinned compiler and its TS 6.0 and TS 5.9 probes", () => {
    const adapter = join(repoRoot, "packages", "zfb-adapter-cloudflare");
    const primary = resolvePackageTsc(adapter);
    const probe60 = resolvePackageTsc(adapter, "typescript-6.0");
    const probe59 = resolvePackageTsc(adapter, "typescript-5.9");
    expect(primary.version).toBe(
      createRequire(join(adapter, "package.json"))("./package.json").devDependencies.typescript,
    );
    expect(probe60.version).toBe("6.0.3");
    expect(probe59.version).toBe("5.9.3");
    expect(new Set([primary.binPath, probe60.binPath, probe59.binPath]).size).toBe(3);
  });
});
