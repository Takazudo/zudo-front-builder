// scripts/release/npm-package-state.mjs: per-package registry state bound to
// the release source commit (#4690). In-process with an injected HTTP reader;
// the attestation fixture is a trimmed real registry capture.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import { describe, expect, it } from "vite-plus/test";
import {
  collectStates,
  darwinDigestProblems,
  expectationShape,
  main,
  packageState,
  parseArgs,
  provenanceIdentity,
} from "../release/npm-package-state.mjs";
import { scriptsDir } from "./helpers/darwin-fixture.mjs";

const REG = "https://registry.test";
// Real @takazudo/zfb@6.0.1 (registry, 2026-11-12).
const REAL = {
  name: "@takazudo/zfb",
  version: "6.0.1",
  commit: "e6bd0a7c6864a2211fe4d846ec8511b2b7f300bd",
  integrity:
    "sha512-ApV742DBH847yPpLHIpEfpx+mTr1pKX5SxQU76p9sImAH0rhC9Yvw9Exh0APzqLs2UVs/bFiUYzpJa1LrN9pKg==",
};
const FIXTURE = readFileSync(
  join(scriptsDir, "__tests__/fixtures/npm-attestations-zfb-6.0.1.json"),
  "utf8",
);
const OTHER = "f".repeat(40);

const ok = (body) => ({
  status: 200,
  body: Buffer.isBuffer(body)
    ? body
    : Buffer.from(typeof body === "string" ? body : JSON.stringify(body)),
});
const notFound = () => ({ status: 404, body: Buffer.from('"version not found"') });

function tarGz(files) {
  const parts = [];
  for (const [name, data] of Object.entries(files)) {
    const bytes = Buffer.from(data);
    const h = Buffer.alloc(512);
    h.write(`package/${name}`, 0);
    h.write("0000755\0", 100);
    h.write("0000000\0", 108);
    h.write("0000000\0", 116);
    h.write(`${bytes.length.toString(8).padStart(11, "0")}\0`, 124);
    h.write("00000000000\0", 136);
    h.write("        ", 148);
    h.write("0", 156);
    h.write("ustar\0", 257);
    h.write("00", 263);
    let sum = 0;
    for (const b of h) sum += b;
    h.write(`${sum.toString(8).padStart(6, "0")}\0 `, 148);
    parts.push(h, bytes, Buffer.alloc((512 - (bytes.length % 512)) % 512));
  }
  parts.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(parts));
}
const integrityOf = (bytes) => `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
const sha256 = (data) => createHash("sha256").update(Buffer.from(data)).digest("hex");

/** A fake registry: routes keyed by URL → response or function. */
function registry(routes) {
  const calls = [];
  const request = async (url) => {
    calls.push(url);
    const route = routes[url];
    if (route === undefined) return notFound();
    if (typeof route === "function") return route(url);
    return route;
  };
  return { request, calls };
}
const versionUrl = (name, version) =>
  `${REG}/${name.startsWith("@") ? `@${encodeURIComponent(name.slice(1))}` : name}/${version}`;
const ctx = (request, extra = {}) => ({
  version: REAL.version,
  sourceCommit: REAL.commit,
  verdict: null,
  registry: REG,
  request,
  absentReads: 3,
  delayMs: 0,
  ...extra,
});

function attestedRecord(overrides = {}) {
  return {
    name: REAL.name,
    version: REAL.version,
    dist: {
      integrity: REAL.integrity,
      tarball: `${REG}/zfb.tgz`,
      attestations: {
        url: `${REG}/att/zfb`,
        provenance: { predicateType: "https://slsa.dev/provenance/v1" },
      },
    },
    ...overrides,
  };
}

describe("provenanceIdentity (real 6.0.1 bundle)", () => {
  it("decodes the DSSE payload's subject digest and resolved gitCommit", () => {
    const identity = provenanceIdentity(JSON.parse(FIXTURE));
    expect(identity.gitCommit).toBe(REAL.commit);
    expect(identity.subjectName).toBe("pkg:npm/%40takazudo/zfb@6.0.1");
    expect(identity.subjectSha512).toBe(
      Buffer.from(REAL.integrity.slice("sha512-".length), "base64").toString("hex"),
    );
  });

  it("refuses a body without exactly one SLSA provenance attestation", () => {
    expect(() => provenanceIdentity({ attestations: [] })).toThrow(/expected one/);
  });
});

describe("packageState", () => {
  it("present-verified by attestation gitCommit (real bundle)", async () => {
    const { request } = registry({
      [versionUrl(REAL.name, REAL.version)]: ok(attestedRecord()),
      [`${REG}/att/zfb`]: ok(FIXTURE),
    });
    const result = await packageState(REAL.name, ctx(request));
    expect(result).toMatchObject({ state: "present-verified", method: "attestation" });
  });

  it("present-unverified when the attestation names another commit", async () => {
    const { request } = registry({
      [versionUrl(REAL.name, REAL.version)]: ok(attestedRecord()),
      [`${REG}/att/zfb`]: ok(FIXTURE),
    });
    const result = await packageState(REAL.name, ctx(request, { sourceCommit: OTHER }));
    expect(result.state).toBe("present-unverified");
    expect(result.detail).toContain(`attested to commit ${REAL.commit}`);
  });

  it("present-unverified when the attestation subject is not this tarball", async () => {
    const { request } = registry({
      [versionUrl(REAL.name, REAL.version)]: ok(
        attestedRecord({
          dist: { ...attestedRecord().dist, integrity: integrityOf(Buffer.from("x")) },
        }),
      ),
      [`${REG}/att/zfb`]: ok(FIXTURE),
    });
    expect((await packageState(REAL.name, ctx(request))).detail).toContain("subject digest");
  });

  it("uncertain when the attestation bundle cannot be read", async () => {
    const { request } = registry({
      [versionUrl(REAL.name, REAL.version)]: ok(attestedRecord()),
      [`${REG}/att/zfb`]: { status: 502, body: Buffer.from("bad gateway") },
    });
    const result = await packageState(REAL.name, ctx(request));
    expect(result.state).toBe("uncertain");
    expect(result.error).toContain("HTTP 502");
  });

  it("present-verified by gitHead when there is no attestation", async () => {
    const { request } = registry({
      [versionUrl("@takazudo/zfb-linux-x64-gnu", REAL.version)]: ok({
        name: "@takazudo/zfb-linux-x64-gnu",
        version: REAL.version,
        gitHead: REAL.commit,
        dist: { integrity: REAL.integrity },
      }),
    });
    const result = await packageState("@takazudo/zfb-linux-x64-gnu", ctx(request));
    expect(result).toMatchObject({ state: "present-verified", method: "gitHead" });
  });

  it("present-unverified for a foreign gitHead or no identity at all", async () => {
    const record = (extra) =>
      ok({
        name: "create-zfb",
        version: REAL.version,
        dist: { integrity: REAL.integrity },
        ...extra,
      });
    let { request } = registry({
      [versionUrl("create-zfb", REAL.version)]: record({ gitHead: OTHER }),
    });
    expect((await packageState("create-zfb", ctx(request))).state).toBe("present-unverified");
    ({ request } = registry({ [versionUrl("create-zfb", REAL.version)]: record({}) }));
    const none = await packageState("create-zfb", ctx(request));
    expect(none.state).toBe("present-unverified");
    expect(none.detail).toContain("no attestation, gitHead or admitted digest");
  });

  it("absent only after every bounded re-read answers 404", async () => {
    const { request, calls } = registry({});
    const result = await packageState("create-zfb", ctx(request));
    expect(result.state).toBe("absent");
    expect(calls).toHaveLength(3);
  });

  it("a 404 followed by a 200 (registry lag) is present, not absent", async () => {
    let n = 0;
    const { request } = registry({
      [versionUrl("create-zfb", REAL.version)]: () =>
        ++n === 1
          ? notFound()
          : ok({ name: "create-zfb", version: REAL.version, gitHead: REAL.commit, dist: {} }),
    });
    expect((await packageState("create-zfb", ctx(request))).state).toBe("present-verified");
  });

  it.each([
    ["a 5xx", () => ({ status: 503, body: Buffer.from("unavailable") }), /HTTP 503/],
    ["an auth failure", () => ({ status: 401, body: Buffer.from("unauthorized") }), /HTTP 401/],
    [
      "a network error",
      () => {
        throw new Error("GET x failed: ECONNRESET");
      },
      /ECONNRESET/,
    ],
    ["malformed JSON", () => ({ status: 200, body: Buffer.from("{nope") }), /malformed JSON/],
  ])("uncertain (never absent) on %s, keeping the original error", async (_label, route, error) => {
    const { request, calls } = registry({ [versionUrl("create-zfb", REAL.version)]: route });
    const result = await packageState("create-zfb", ctx(request));
    expect(result.state).toBe("uncertain");
    expect(result.error).toMatch(error);
    expect(calls).toHaveLength(1);
  });

  describe("Darwin carrier against the admission verdict", () => {
    const name = "@takazudo/zfb-darwin-x64";
    const binary = "admitted zfb bytes";
    const addon = "admitted addon bytes";
    const verdict = (b = binary, a = addon) => ({
      admitted: true,
      targets: {
        "x86_64-apple-darwin": { binary: { sha256: sha256(b) }, addon: { sha256: sha256(a) } },
      },
    });
    const setup = (files, extra = {}) => {
      const tarball = tarGz(files);
      return registry({
        [versionUrl(name, REAL.version)]: ok({
          name,
          version: REAL.version,
          gitHead: REAL.commit,
          dist: { integrity: integrityOf(tarball), tarball: `${REG}/carrier.tgz` },
          ...extra,
        }),
        [`${REG}/carrier.tgz`]: ok(tarball),
      }).request;
    };

    it("verified when gitHead AND the packed bytes match the admitted digests", async () => {
      const request = setup({ "package.json": "{}", zfb: binary, "zfb-css.node": addon });
      const result = await packageState(name, ctx(request, { verdict: verdict() }));
      expect(result).toMatchObject({ state: "present-verified", method: "gitHead+darwin-digest" });
    });

    it("unverified when the packed binary differs, even though gitHead matches", async () => {
      const request = setup({ "package.json": "{}", zfb: "other bytes", "zfb-css.node": addon });
      const result = await packageState(name, ctx(request, { verdict: verdict() }));
      expect(result.state).toBe("present-unverified");
      expect(result.detail).toContain("package/zfb sha256");
    });

    it("verified by admitted digests alone when no gitHead was recorded", async () => {
      const request = setup(
        { "package.json": "{}", zfb: binary, "zfb-css.node": addon },
        { gitHead: undefined },
      );
      const result = await packageState(name, ctx(request, { verdict: verdict() }));
      expect(result).toMatchObject({ state: "present-verified", method: "darwin-digest" });
    });

    it("reports digest problems for a missing addon or a non-admitted verdict", () => {
      expect(darwinDigestProblems([], { admitted: false }, "x86_64-apple-darwin")[0]).toContain(
        "no admitted",
      );
      const files = [{ name: "package/zfb", data: Buffer.from(binary) }];
      expect(darwinDigestProblems(files, verdict(), "x86_64-apple-darwin")).toEqual([
        "package/zfb-css.node is missing from the packed carrier",
      ]);
    });
  });
});

describe("collectStates / expectations / CLI", () => {
  it("summarizes a partial publish: some verified, one absent, one unverified", async () => {
    const { request } = registry({
      [versionUrl("@takazudo/zfb", REAL.version)]: ok(attestedRecord()),
      [`${REG}/att/zfb`]: ok(FIXTURE),
      [versionUrl("create-zfb", REAL.version)]: ok({
        name: "create-zfb",
        version: REAL.version,
        gitHead: OTHER,
        dist: {},
      }),
    });
    const report = await collectStates({
      version: REAL.version,
      sourceCommit: REAL.commit,
      packages: ["@takazudo/zfb", "create-zfb", "@takazudo/zfb-runtime"],
      registry: REG,
      request,
      delayMs: 0,
    });
    expect(report.summary).toMatchObject({
      "present-verified": ["@takazudo/zfb"],
      "present-unverified": ["create-zfb"],
      absent: ["@takazudo/zfb-runtime"],
    });
    expect(report.allVerified).toBe(false);
  });

  it("expectation shape: normal (Darwin none, rest attested) vs recovery (all none)", () => {
    const normal = expectationShape([
      { name: "@takazudo/zfb", attested: true },
      { name: "@takazudo/zfb-darwin-arm64", attested: false },
    ]);
    expect(normal.map((e) => e.expected)).toEqual(["attested", "none"]);
    const recovery = expectationShape([
      { name: "@takazudo/zfb", attested: false },
      { name: "@takazudo/zfb-darwin-arm64", attested: false },
    ]);
    expect(recovery.map((e) => e.expected)).toEqual(["none", "none"]);
  });

  it("parses arguments strictly", () => {
    expect(parseArgs(["--version", "1.0.0", "--source-commit", REAL.commit])).toMatchObject({
      version: "1.0.0",
    });
    expect(parseArgs(["--version", "1.0.0", "--source-commit", "abc"])).toBeNull();
    expect(
      parseArgs(["--version", "1.0.0", "--source-commit", REAL.commit, "--package", "left-pad"]),
    ).toBeNull();
  });

  it("prints one TSV line per package and honors --require-all-verified", async () => {
    let text = "";
    const out = { write: (chunk) => (text += chunk) };
    const { request } = registry({});
    const args = [
      "--version",
      REAL.version,
      "--source-commit",
      REAL.commit,
      "--package",
      "create-zfb",
      "--tsv",
    ];
    const env = { ZFB_NPM_REGISTRY: REG, ZFB_NPM_STATE_DELAY_MS: "0" };
    expect(await main(args, { env, out, request })).toBe(0);
    expect(text).toMatch(/^create-zfb\tabsent\tE404 on 3 read\(s\)/);
    expect(await main([...args, "--require-all-verified"], { env, out, request })).toBe(1);
    expect(await main(["--version", REAL.version], { env, out, request })).toBe(2);
  });
});
