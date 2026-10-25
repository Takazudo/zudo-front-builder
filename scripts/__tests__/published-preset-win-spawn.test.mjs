import { readFileSync } from "node:fs";
import { describe, expect, it } from "vite-plus/test";
import {
  locateNpmCli,
  npmCommand,
  zudoSgCommand,
} from "../../tests/published-preset-identity/carrier.mjs";

const FIXTURE_DIR = new URL("../../tests/published-preset-identity/", import.meta.url);

const WIN_NODE = "C:\\Program Files\\nodejs\\node.exe";
const WIN_CLI = "C:\\Program Files\\nodejs\\node_modules\\npm\\bin\\npm-cli.js";
const POSIX_NODE = "/opt/my node/bin/node";
const POSIX_CLI = "/opt/my node/lib/node_modules/npm/bin/npm-cli.js";

const only =
  (...found) =>
  (candidate) =>
    found.includes(candidate);

describe("locateNpmCli", () => {
  it("resolves next to node.exe on win32 (path with spaces)", () => {
    expect(
      locateNpmCli({ platform: "win32", execPath: WIN_NODE, env: {}, exists: only(WIN_CLI) }),
    ).toBe(WIN_CLI);
  });

  it("resolves <prefix>/lib/node_modules on POSIX (path with spaces)", () => {
    expect(
      locateNpmCli({ platform: "linux", execPath: POSIX_NODE, env: {}, exists: only(POSIX_CLI) }),
    ).toBe(POSIX_CLI);
  });

  it("prefers npm_execpath when it is an existing npm-cli.js", () => {
    const fromEnv = "D:\\tools dir\\npm\\bin\\npm-cli.js";
    expect(
      locateNpmCli({
        platform: "win32",
        execPath: WIN_NODE,
        env: { npm_execpath: fromEnv },
        exists: only(fromEnv, WIN_CLI),
      }),
    ).toBe(fromEnv);
  });

  it("ignores npm_execpath that is not npm-cli.js (e.g. pnpm)", () => {
    expect(
      locateNpmCli({
        platform: "linux",
        execPath: POSIX_NODE,
        env: { npm_execpath: "/usr/lib/pnpm/bin/pnpm.cjs" },
        exists: only("/usr/lib/pnpm/bin/pnpm.cjs", POSIX_CLI),
      }),
    ).toBe(POSIX_CLI);
  });

  it("fails clearly when npm-cli.js cannot be found", () => {
    expect(() =>
      locateNpmCli({ platform: "win32", execPath: WIN_NODE, env: {}, exists: () => false }),
    ).toThrow(/npm-cli\.js not found.*npm-cli\.js/s);
  });
});

describe("npmCommand", () => {
  it("builds node + npm-cli.js argv per platform", () => {
    expect(
      npmCommand(["ci", "--ignore-scripts"], {
        platform: "win32",
        execPath: WIN_NODE,
        env: {},
        exists: only(WIN_CLI),
      }),
    ).toEqual({ command: WIN_NODE, args: [WIN_CLI, "ci", "--ignore-scripts"] });
    expect(
      npmCommand(["--version"], {
        platform: "darwin",
        execPath: POSIX_NODE,
        env: {},
        exists: only(POSIX_CLI),
      }),
    ).toEqual({ command: POSIX_NODE, args: [POSIX_CLI, "--version"] });
  });
});

describe("zudoSgCommand", () => {
  const manifestFor = (bin) => async () => JSON.stringify({ name: "@takazudo/zudo-sg", bin });

  it("runs the package bin entry under node on win32 (root with spaces)", async () => {
    const root = "C:\\Users\\Jane Doe\\install root";
    const seen = [];
    const cmd = await zudoSgCommand(root, ["gen-registry", "--check"], {
      platform: "win32",
      execPath: WIN_NODE,
      readText: async (file) => {
        seen.push(file);
        return JSON.stringify({ bin: { "zudo-sg": "./dist/cli.js" } });
      },
    });
    expect(seen).toEqual([`${root}\\node_modules\\@takazudo\\zudo-sg\\package.json`]);
    expect(cmd).toEqual({
      command: WIN_NODE,
      args: [`${root}\\node_modules\\@takazudo\\zudo-sg\\dist\\cli.js`, "gen-registry", "--check"],
    });
  });

  it("supports a string bin on POSIX (root with spaces)", async () => {
    const root = "/tmp/my install";
    const cmd = await zudoSgCommand(root, ["gen-token-manifest", "--check"], {
      platform: "linux",
      execPath: POSIX_NODE,
      readText: manifestFor("bin/zudo-sg.mjs"),
    });
    expect(cmd).toEqual({
      command: POSIX_NODE,
      args: [
        `${root}/node_modules/@takazudo/zudo-sg/bin/zudo-sg.mjs`,
        "gen-token-manifest",
        "--check",
      ],
    });
  });

  it("rejects a manifest without a zudo-sg bin entry", async () => {
    await expect(
      zudoSgCommand("/x", [], { platform: "linux", readText: manifestFor({ other: "a.js" }) }),
    ).rejects.toThrow(/no "zudo-sg" bin entry/);
  });
});

describe("fixture spawns", () => {
  it("never uses shell: true", () => {
    for (const file of ["run.mjs", "carrier.mjs"]) {
      expect(readFileSync(new URL(file, FIXTURE_DIR), "utf8")).not.toMatch(/shell\s*:\s*true/);
    }
  });

  it("run.mjs no longer spawns bare npm or the .bin shim", () => {
    const source = readFileSync(new URL("run.mjs", FIXTURE_DIR), "utf8");
    expect(source).toContain("npmCommand(args)");
    expect(source).not.toMatch(/(?:runLogged\(\s*|command\s*=\s*)"npm"/);
    expect(source).not.toMatch(/"\.bin",\s*"zudo-sg"/);
  });
});
