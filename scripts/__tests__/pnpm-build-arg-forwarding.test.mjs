import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// Cheap regression test for the wiring `pnpm test:md-wasm:local` depends on
// (zfb#3060): `pnpm --filter <pkg> build --allow-over-ceiling` must forward
// `--allow-over-ceiling` into the `build` script's own argv. This proves the
// pnpm arg-forwarding mechanism itself, in a throwaway workspace fixture,
// without building zfb-md-wasm's wasm artifacts. The fixture is a real
// two-package workspace so the `--filter` form -- the one package.json
// actually uses -- is what gets exercised, not just a bare `pnpm build`.

const PACKAGE_NAME = "@zfb-fixture/arg-forwarding";
const temporaryDirectories = [];

function makeWorkspaceFixture() {
  const directory = mkdtempSync(join(tmpdir(), "zfb-pnpm-arg-forwarding-"));
  temporaryDirectories.push(directory);

  writeFileSync(join(directory, "pnpm-workspace.yaml"), 'packages:\n  - "packages/*"\n');
  writeFileSync(
    join(directory, "package.json"),
    JSON.stringify({ name: "zfb-arg-forwarding-root", private: true }),
  );

  const packageDirectory = join(directory, "packages", "fixture");
  mkdirSync(packageDirectory, { recursive: true });
  writeFileSync(
    join(packageDirectory, "package.json"),
    JSON.stringify({
      name: PACKAGE_NAME,
      private: true,
      scripts: { build: "node print-argv.mjs" },
    }),
  );
  writeFileSync(
    join(packageDirectory, "print-argv.mjs"),
    'console.log("ARGV=" + JSON.stringify(process.argv.slice(2)));',
  );

  return directory;
}

function runPnpm(directory, args) {
  return execFileSync("pnpm", args, { cwd: directory, encoding: "utf8" });
}

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
  }
});

describe("pnpm build argument forwarding", () => {
  it("delivers --allow-over-ceiling into the filtered build script's argv", () => {
    const directory = makeWorkspaceFixture();
    const output = runPnpm(directory, ["--filter", PACKAGE_NAME, "build", "--allow-over-ceiling"]);

    expect(output).toContain('ARGV=["--allow-over-ceiling"]');
  });

  it("delivers no extra argv when the flag is omitted", () => {
    const directory = makeWorkspaceFixture();
    const output = runPnpm(directory, ["--filter", PACKAGE_NAME, "build"]);

    expect(output).toContain("ARGV=[]");
    expect(output).not.toContain("--allow-over-ceiling");
  });
});
