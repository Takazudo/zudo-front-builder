import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// Cheap regression test for the wiring `pnpm test:md-wasm:local` depends on
// (zfb#3060): `pnpm --filter <pkg> build --allow-over-ceiling` must forward
// `--allow-over-ceiling` into the `build` script's own argv. This proves the
// pnpm arg-forwarding mechanism itself, in a throwaway single-package
// fixture, without building zfb-md-wasm's wasm artifacts.

const temporaryDirectories = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("pnpm build argument forwarding", () => {
  it("delivers --allow-over-ceiling into the build script's argv", () => {
    const directory = mkdtempSync(join(tmpdir(), "zfb-pnpm-arg-forwarding-"));
    temporaryDirectories.push(directory);

    writeFileSync(
      join(directory, "package.json"),
      JSON.stringify({
        name: "zfb-arg-forwarding-fixture",
        private: true,
        scripts: { build: "node print-argv.mjs" },
      }),
    );
    writeFileSync(
      join(directory, "print-argv.mjs"),
      "console.log(JSON.stringify(process.argv.slice(2)));",
    );

    const output = execFileSync("pnpm", ["build", "--allow-over-ceiling"], {
      cwd: directory,
      encoding: "utf8",
    });

    expect(output).toContain('["--allow-over-ceiling"]');
  });

  it("delivers no extra argv when the flag is omitted", () => {
    const directory = mkdtempSync(join(tmpdir(), "zfb-pnpm-arg-forwarding-"));
    temporaryDirectories.push(directory);

    writeFileSync(
      join(directory, "package.json"),
      JSON.stringify({
        name: "zfb-arg-forwarding-fixture",
        private: true,
        scripts: { build: "node print-argv.mjs" },
      }),
    );
    writeFileSync(
      join(directory, "print-argv.mjs"),
      "console.log(JSON.stringify(process.argv.slice(2)));",
    );

    const output = execFileSync("pnpm", ["build"], {
      cwd: directory,
      encoding: "utf8",
    });

    expect(output).toContain("[]");
    expect(output).not.toContain("--allow-over-ceiling");
  });
});
