import { describe, it, expect } from "vite-plus/test";
import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
const root = resolve(import.meta.dirname, "../..");
const simulated =
  'Object.defineProperty(process,"platform",{value:"linux"});Object.defineProperty(process,"arch",{value:"arm64"});';
describe("Linux arm64 npm suspension", () => {
  it("keeps CSS import lazy and rejects native use with a suspension cause", () => {
    const url = pathToFileURL(join(root, "packages/zfb/src/css.ts")).href;
    const code = `${simulated}const {compileCss}=await import(${JSON.stringify(url)});try{await compileCss({input:"input.css"});process.exit(2)}catch(e){if(!/Linux arm64.*suspended/.test(e.message))throw e;console.log(e.message)}`;
    const result = spawnSync(process.execPath, ["--input-type=module", "-e", code], {
      encoding: "utf8",
    });
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).not.toMatch(/optional dependencies/i);
  });
  it("repeated version sync removes suspended dependency while stamping non-npm carrier", () => {
    const room = mkdtempSync(join(tmpdir(), "zfb-sync-policy-"));
    try {
      const files = execFileSync(
        "git",
        ["ls-files", "packages/*/package.json", "crates/zfb-md-wasm/npm/package.json"],
        { cwd: root, encoding: "utf8" },
      )
        .trim()
        .split("\n");
      files.push(
        "scripts/sync-platform-versions.mjs",
        "scripts/release/npm-publication-policy.mjs",
        "scripts/release/gh.mjs",
        "release/npm-publication-policy.json",
      );
      for (const file of files) {
        mkdirSync(dirname(join(room, file)), { recursive: true });
        cpSync(join(root, file), join(room, file));
      }
      const sdkPath = join(room, "packages/zfb/package.json");
      const sdk = JSON.parse(readFileSync(sdkPath, "utf8"));
      sdk.version = "99.0.0";
      sdk.optionalDependencies["@takazudo/zfb-linux-arm64-gnu"] = "workspace:6.0.1";
      writeFileSync(sdkPath, JSON.stringify(sdk));
      for (let i = 0; i < 2; i++)
        execFileSync(process.execPath, [join(room, "scripts/sync-platform-versions.mjs")]);
      expect(JSON.parse(readFileSync(sdkPath, "utf8")).optionalDependencies).not.toHaveProperty(
        "@takazudo/zfb-linux-arm64-gnu",
      );
      expect(
        JSON.parse(readFileSync(join(room, "packages/zfb-linux-arm64-gnu/package.json"), "utf8"))
          .version,
      ).toBe("99.0.0");
    } finally {
      rmSync(room, { recursive: true, force: true });
    }
  });
});
