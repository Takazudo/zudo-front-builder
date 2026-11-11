// Tests for scripts/npm-oidc-preflight.mjs — the fail-closed guard every
// release-path registry write runs first. Every dependency (env, files, npm) is
// injected, so no test reads this machine's real npm config.

import { describe, expect, it } from "vite-plus/test";

import {
  authKeysInIni,
  inspectEnv,
  npmOidcPreflight,
  versionAtLeast,
} from "../npm-oidc-preflight.mjs";

const OIDC = {
  ACTIONS_ID_TOKEN_REQUEST_URL: "https://oidc.invalid/token",
  ACTIONS_ID_TOKEN_REQUEST_TOKEN: "dummy",
};

function run({
  env = OIDC,
  files = {},
  configList = "",
  npmVersion = "12.2.1",
  npmFails = false,
} = {}) {
  return npmOidcPreflight({
    env: { HOME: "/home/runner", ...env },
    cwd: "/work/repo/packages/zfb",
    home: "/home/runner",
    fileExists: (p) => Object.hasOwn(files, p),
    readFile: (p) => files[p],
    npm: (args) => {
      if (npmFails) throw new Error("spawn npm ENOENT");
      if (args[0] === "--version") return `${npmVersion}\n`;
      if (args[0] === "config") return configList;
      throw new Error(`unexpected npm call ${args.join(" ")}`);
    },
  });
}

describe("npmOidcPreflight", () => {
  it("is clear with GitHub OIDC available, no credentials anywhere, and npm 12.2.1", () => {
    expect(run()).toEqual([]);
  });

  it.each(OIDC_KEYS())("refuses when %s is missing (no id-token: write)", (name) => {
    const env = { ...OIDC };
    delete env[name];
    expect(run({ env }).join("\n")).toMatch(new RegExp(`${name} is not set`));
  });

  it.each([
    ["NODE_AUTH_TOKEN", "x"],
    ["NODE_AUTH_TOKEN", ""], // defined-but-empty is still refused: it is a token slot
    ["NPM_TOKEN", "x"],
    ["NPM_CONFIG__AUTH", "x"],
    ["npm_config__authToken", "x"],
    ["npm_config_//registry.npmjs.org/:_authToken", "x"],
    ["PNPM_CONFIG__AUTH", "x"],
  ])("refuses a stored-credential env var %s", (name, value) => {
    expect(run({ env: { ...OIDC, [name]: value } }).join("\n")).toContain(name);
  });

  it("ignores unrelated npm_config_* variables", () => {
    expect(run({ env: { ...OIDC, npm_config_registry: "https://registry.npmjs.org/" } })).toEqual(
      [],
    );
  });

  it("refuses an _authToken in an .npmrc above the working directory (setup-node style)", () => {
    const problems = run({
      files: { "/work/repo/.npmrc": "//registry.npmjs.org/:_authToken=${NODE_AUTH_TOKEN}\n" },
    });
    expect(problems.join("\n")).toMatch(
      /\/work\/repo\/\.npmrc defines \/\/registry\.npmjs\.org\/:_authToken/,
    );
  });

  it("refuses credentials in NPM_CONFIG_USERCONFIG and pnpm's auth.ini", () => {
    const problems = run({
      env: { ...OIDC, NPM_CONFIG_USERCONFIG: "/tmp/runner/.npmrc" },
      files: {
        "/tmp/runner/.npmrc": "_auth=abc\n",
        "/home/runner/.config/pnpm/auth.ini": "//registry.npmjs.org/:_authToken=abc\n",
      },
    });
    expect(problems.join("\n")).toMatch(/\/tmp\/runner\/\.npmrc defines _auth/);
    expect(problems.join("\n")).toMatch(/auth\.ini defines/);
  });

  it("refuses auth keys reported by `npm config list` (redacted as (protected))", () => {
    const configList = [
      '; "user" config from /home/runner/.npmrc',
      "//registry.npmjs.org/:_authToken = (protected)",
      '; "env" config from environment',
      "; _auth = (protected) ; overridden by env",
    ].join("\n");
    const problems = run({ configList });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/effective config contains \/\/registry\.npmjs\.org\/:_authToken/);
  });

  it("fails closed when npm cannot be consulted", () => {
    expect(run({ npmFails: true }).join("\n")).toMatch(/npm config list` failed/);
  });

  it("refuses an npm older than 12.2.0 (no OIDC for dist-tag writes)", () => {
    expect(run({ npmVersion: "11.20.0" }).join("\n")).toMatch(
      /npm 11\.20\.0 is older than 12\.2\.0/,
    );
  });

  it("never echoes a credential value", () => {
    const problems = run({
      env: { ...OIDC, NPM_TOKEN: "npm_SECRETVALUE" },
      files: { "/work/repo/.npmrc": "//registry.npmjs.org/:_authToken=npm_OTHERSECRET\n" },
    });
    expect(problems.join("\n")).not.toMatch(/SECRETVALUE|OTHERSECRET/);
  });
});

function OIDC_KEYS() {
  return Object.keys(OIDC);
}

describe("helpers", () => {
  it("parses ini, npm config list and yaml forms of auth keys", () => {
    expect(authKeysInIni("registry=https://x/\n//r.example/:_authToken=abc\n")).toEqual([
      "//r.example/:_authToken",
    ]);
    expect(authKeysInIni('"//r.example/:_authToken": abc\n')).toEqual(["//r.example/:_authToken"]);
    expect(authKeysInIni("; //r/:_authToken=commented\n# _auth=x\n")).toEqual([]);
  });

  it("inspectEnv only flags credential-shaped names", () => {
    expect(inspectEnv({ ...OIDC, npm_config_provenance: "false" })).toEqual([]);
  });

  it("compares npm versions", () => {
    expect(versionAtLeast("12.2.1", "12.2.0")).toBe(true);
    expect(versionAtLeast("12.2.0", "12.2.0")).toBe(true);
    expect(versionAtLeast("12.1.9", "12.2.0")).toBe(false);
    expect(versionAtLeast("", "12.2.0")).toBe(false);
  });
});
