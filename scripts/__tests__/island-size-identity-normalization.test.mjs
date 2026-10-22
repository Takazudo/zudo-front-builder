// Identity normalization over real emitted island bytes (#4131).
//
// L1: the normalization boundaries on the compiler-owned identity sites.
// L3: tamper and negative controls over the bytes the v4.3.1 Linux x64
// artifact actually shipped (health run 38075998971), copied verbatim into
// fixtures/island-size/ — every number is recomputed from those bytes.
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { afterEach, describe, expect, it } from "vite-plus/test";

import {
  IDENTITY_NORMALIZATION_RULES,
  IDENTITY_NORMALIZATION_VERSION,
  IDENTITY_NORMALIZED_METRIC,
  IDENTITY_PLACEHOLDER,
  IDENTITY_SITES_BY_ROLE,
  NORMALIZER_FILE,
  buildTokenFromHtml,
  normalizeFixtureFiles,
  normalizeIdentitySites,
  normalizeMeasurement,
  normalizeMeasurementDirectory,
  normalizerSha256,
} from "../../research/v3-island-size/normalize-identity.mjs";

const testDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(testDir, "../..");
const fixtureRoot = join(testDir, "fixtures/island-size");
const linuxContract = JSON.parse(
  readFileSync(join(repoRoot, "research/v3-island-size/decision-linux-x64.json"), "utf8"),
);
// The v4.3.1 fixture bytes always measure these totals; they equal the Linux ceilings
// recorded for v4.3.1 and are no longer the live contract ceilings after #4203 lowered them.
const V431_MULTI_ISLAND = { raw: 67072, gzipNormalized: 22324 };
const ENTRY = "assets/islands-dca258ba.js";
const REAL_TOKEN = "d1cbd3e6cdd9d75e";
const tempRoots = [];

function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function gzip(bytes) {
  return gzipSync(bytes, { level: 9, mtime: 0 }).length;
}

function readTree(directory) {
  const files = {};
  const visit = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const absolute = join(current, entry.name);
      if (entry.isDirectory()) visit(absolute);
      else files[relative(directory, absolute).replaceAll("\\", "/")] = readFileSync(absolute);
    }
  };
  visit(directory);
  return files;
}

function fixtureFiles(name) {
  return readTree(join(fixtureRoot, name));
}

function clone(files) {
  return Object.fromEntries(
    Object.entries(files).map(([path, bytes]) => [path, Buffer.from(bytes)]),
  );
}

function retoken(files, from, to) {
  const next = clone(files);
  for (const path of Object.keys(next)) {
    if (path === "index.html" || path.endsWith(".js")) {
      next[path] = Buffer.from(next[path].toString("latin1").replaceAll(from, to), "latin1");
    }
  }
  return next;
}

// Independent claim builder, the shape measure-real.mjs writes; deliberately
// not derived from the normalizer so a disagreement is detectable.
function claimedResult(files) {
  const paths = Object.keys(files).sort();
  const jsPaths = paths.filter((path) => /\.(?:js|mjs|cjs)$/.test(path));
  const entryPaths = jsPaths.filter(
    (path) =>
      /^assets\/islands(?:-[^/]+)?\.js$/.test(path) && !path.startsWith("assets/islands-chunk-"),
  );
  const initial = new Set();
  const visit = (path) => {
    if (initial.has(path)) return;
    initial.add(path);
    for (const match of files[path]
      .toString("utf8")
      .matchAll(/\bimport\s*(?!\()(?:[^;'"()]*?\bfrom\s*)?["'](\.\.?\/[^"']+\.js)["']/g)) {
      const target = join(dirname(path), match[1]).replaceAll("\\", "/");
      if (files[target]) visit(target);
    }
  };
  for (const path of entryPaths) visit(path);
  const chunks = jsPaths.map((file) => ({
    file,
    phase: initial.has(file) ? "initial" : "later",
    role: entryPaths.includes(file) ? "entry" : initial.has(file) ? "shared" : "lazy",
    raw: files[file].length,
    gzip: gzip(files[file]),
    sha256: sha(files[file]),
  }));
  const sum = (rows) => ({
    raw: rows.reduce((total, chunk) => total + chunk.raw, 0),
    gzip: rows.reduce((total, chunk) => total + chunk.gzip, 0),
  });
  const inventory = paths.map((path) => ({ path, sha256: sha(files[path]) }));
  return {
    files: inventory,
    chunks,
    entryPaths,
    initial: sum(chunks.filter((chunk) => chunk.phase === "initial")),
    later: sum(chunks.filter((chunk) => chunk.phase === "later")),
    shipped: sum(chunks),
    repeatVerification: {
      passes: 2,
      inventoriesIdentical: true,
      inventorySha256: sha(Buffer.from(JSON.stringify(inventory))),
    },
  };
}

function measurementFor(cases) {
  const results = {};
  const artifacts = {};
  for (const [name, files] of Object.entries(cases)) {
    results[name] = claimedResult(files);
    artifacts[name] = { passes: { 1: { files }, 2: { files: clone(files) } } };
  }
  const measurement = {
    provenance: {
      mode: "workspace",
      sourceSha: "3aaad9b4c403a3c8e920384885c541d5864d9c00",
      platform: { os: "linux", arch: "x64" },
      packageVersion: "4.3.1",
      nodeVersion: "v24.14.0",
      zlibVersion: "1.3.1-e00f703",
      compression: linuxContract.compression,
      runnerSha256: linuxContract.runnerSha256,
    },
    results,
  };
  return { measurementText: JSON.stringify(measurement, null, 2) + "\n", artifacts };
}

function normalize(cases) {
  return normalizeMeasurement(measurementFor(cases));
}

function writeMeasurementDirectory(cases) {
  const root = mkdtempSync(join(tmpdir(), "zfb-island-normalize-test-"));
  tempRoots.push(root);
  const { measurementText, artifacts } = measurementFor(cases);
  writeFileSync(join(root, "measurement.json"), measurementText);
  writeFileSync(join(root, "report.md"), "synthetic\n");
  for (const [name, artifact] of Object.entries(artifacts)) {
    for (const passNumber of ["1", "2"]) {
      for (const [path, bytes] of Object.entries(artifact.passes[passNumber].files)) {
        const absolute = join(root, name, "pass-" + passNumber, path);
        mkdirSync(dirname(absolute), { recursive: true });
        writeFileSync(absolute, bytes);
      }
    }
  }
  return root;
}

afterEach(() => {
  for (const root of tempRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("identity normalization boundaries (L1)", () => {
  it("pins the inventory helper with the normalizer in the Linux contract", () => {
    const sizeDir = join(repoRoot, "research/v3-island-size");
    const expected = createHash("sha256")
      .update("normalize-identity.mjs\0")
      .update(readFileSync(join(sizeDir, "normalize-identity.mjs")))
      .update("\0chunk-inventory.mjs\0")
      .update(readFileSync(join(sizeDir, "chunk-inventory.mjs")))
      .digest("hex");
    expect(normalizerSha256()).toBe(expected);
    expect(linuxContract.normalizerSha256).toBe(expected);
    expect(expected).not.toBe(sha(readFileSync(join(sizeDir, "normalize-identity.mjs"))));
  });

  it("reads one 16-hex token from the real HTML and rejects malformed or disagreeing ones", () => {
    const html = fixtureFiles("multi-island")["index.html"].toString("utf8");
    expect(html.match(/data-zfb-build=/g)).toHaveLength(2);
    expect(buildTokenFromHtml(html)).toBe(REAL_TOKEN);
    expect(buildTokenFromHtml("<html><body>no islands</body></html>")).toBeNull();
    expect(() => buildTokenFromHtml("<div data-zfb-build='" + REAL_TOKEN + "'></div>")).toThrow(
      /malformed data-zfb-build attribute/,
    );
    expect(() => buildTokenFromHtml("<div data-zfb-build></div>")).toThrow(
      /malformed data-zfb-build attribute/,
    );
    expect(() => buildTokenFromHtml(html.replace(REAL_TOKEN, "D1CBD3E6CDD9D75E"))).toThrow(
      /data-zfb-build attributes disagree|must be 16 lowercase hex/,
    );
    expect(() => buildTokenFromHtml(html.replaceAll(REAL_TOKEN, "d1cbd3e6cdd9d75"))).toThrow(
      /must be 16 lowercase hex characters, found "d1cbd3e6cdd9d75"/,
    );
    expect(() => buildTokenFromHtml(html.replace(REAL_TOKEN, "0".repeat(16)))).toThrow(
      /data-zfb-build attributes disagree: 0000000000000000, d1cbd3e6cdd9d75e/,
    );
  });

  it("replaces exactly the two compiler-owned sites in the real entry with a same-length placeholder", () => {
    const entry = fixtureFiles("multi-island")[ENTRY];
    expect(IDENTITY_SITES_BY_ROLE).toEqual({ entry: 2, shared: 0, lazy: 0 });
    const { bytes, sites } = normalizeIdentitySites(entry, REAL_TOKEN, 2, ENTRY);
    expect(sites).toBe(2);
    expect(bytes.length).toBe(entry.length);
    expect(bytes.equals(entry)).toBe(false);
    const text = bytes.toString("latin1");
    expect(text.match(new RegExp('build:"' + IDENTITY_PLACEHOLDER + '"', "g"))).toHaveLength(2);
    expect(text.includes(REAL_TOKEN)).toBe(false);
    expect(text).toContain('identity:{component:n,build:"' + IDENTITY_PLACEHOLDER + '"}');
    expect(text).toContain('let l={component:n,build:"' + IDENTITY_PLACEHOLDER + '"');
    // Only the two sites differ; every other byte is untouched.
    let differing = 0;
    for (let index = 0; index < entry.length; index += 1) {
      if (entry[index] !== bytes[index]) differing += 1;
    }
    expect(differing).toBeLessThanOrEqual(32);
  });

  it("rejects a site carrying another token and an unexpected site count", () => {
    const entry = fixtureFiles("multi-island")[ENTRY];
    const other = "0123456789abcdef";
    const oneForeign = Buffer.from(
      entry.toString("latin1").replace('build:"' + REAL_TOKEN + '"', 'build:"' + other + '"'),
      "latin1",
    );
    expect(() => normalizeIdentitySites(oneForeign, REAL_TOKEN, 2, ENTRY)).toThrow(
      "build identity site carries 0123456789abcdef but index.html declares " + REAL_TOKEN,
    );
    expect(() => normalizeIdentitySites(entry, REAL_TOKEN, 1, ENTRY)).toThrow(
      "expected 1 build identity site(s), found 2",
    );
    const stripped = Buffer.from(
      entry.toString("latin1").replaceAll('build:"' + REAL_TOKEN + '"', 'build:""'),
      "latin1",
    );
    expect(() => normalizeIdentitySites(stripped, REAL_TOKEN, 2, ENTRY)).toThrow(
      "expected 2 build identity site(s), found 0",
    );
  });

  it("leaves unrelated 16-hex strings and other build properties alone", () => {
    const unrelated = Buffer.from(
      'const digest="' +
        REAL_TOKEN +
        '";const sha="' +
        "ab".repeat(32) +
        '";const o={builder:"' +
        REAL_TOKEN +
        '",build:' +
        JSON.stringify("not-hex") +
        "};\n",
    );
    const { bytes, sites } = normalizeIdentitySites(unrelated, REAL_TOKEN, 0, "chunk.js");
    expect(sites).toBe(0);
    expect(bytes.equals(unrelated)).toBe(true);
  });

  it("leaves matching build properties in user objects, comments, and strings alone", () => {
    const entry = fixtureFiles("multi-island")[ENTRY];
    const decoys = Buffer.from(
      ';const user={build:"' +
        REAL_TOKEN +
        '"};/* identity:{component:n,build:"' +
        REAL_TOKEN +
        '"} */const text=\'let x={component:n,build:"' +
        REAL_TOKEN +
        "\"}';\n",
    );
    const bytes = Buffer.concat([entry, decoys]);
    const normalized = normalizeIdentitySites(bytes, REAL_TOKEN, 2, ENTRY);
    expect(normalized.sites).toBe(2);
    expect(normalized.bytes.subarray(entry.length).equals(decoys)).toBe(true);
  });

  it("keeps byte positions correct after a non-BMP character", () => {
    const entry = fixtureFiles("multi-island")[ENTRY];
    const prefix = Buffer.from('const symbol="😀";');
    const normalized = normalizeIdentitySites(Buffer.concat([prefix, entry]), REAL_TOKEN, 2, ENTRY);
    expect(normalized.bytes.subarray(0, prefix.length).equals(prefix)).toBe(true);
    expect(normalized.bytes.subarray(prefix.length).toString("utf8")).toContain(
      'identity:{component:n,build:"' + IDENTITY_PLACEHOLDER + '"}',
    );
  });

  it("requires the HTML/bundle relationship per case", () => {
    const island = fixtureFiles("multi-island");
    const none = fixtureFiles("no-island");

    const ok = normalizeFixtureFiles(island, "multi-island");
    expect(ok.buildToken).toBe(REAL_TOKEN);
    expect(ok.chunks).toHaveLength(1);
    expect(ok.chunks[0]).toMatchObject({ file: ENTRY, role: "entry", identitySites: 2 });

    const zero = normalizeFixtureFiles(none, "no-island");
    expect(zero).toMatchObject({
      buildToken: null,
      chunks: [],
      shipped: { raw: 0, gzip: 0, gzipNormalized: 0 },
    });

    const noSites = clone(island);
    noSites[ENTRY] = Buffer.from(
      island[ENTRY].toString("latin1").replaceAll('build:"' + REAL_TOKEN + '"', 'build:""'),
      "latin1",
    );
    expect(() => normalizeFixtureFiles(noSites, "multi-island")).toThrow(
      "expected 2 build identity site(s), found 0",
    );

    const noHtmlToken = clone(island);
    noHtmlToken["index.html"] = Buffer.from(
      island["index.html"].toString("utf8").replaceAll(' data-zfb-build="' + REAL_TOKEN + '"', ""),
    );
    expect(() => normalizeFixtureFiles(noHtmlToken, "multi-island")).toThrow(
      "JavaScript was emitted but index.html declares no build token",
    );

    const tokenWithoutJs = clone(none);
    tokenWithoutJs["index.html"] = Buffer.from(
      '<html><body><div data-zfb-build="' + REAL_TOKEN + '"></div></body></html>',
    );
    expect(() => normalizeFixtureFiles(tokenWithoutJs, "no-island")).toThrow(
      "index.html declares a build token but no JavaScript was emitted",
    );
  });
});

// Normalizer v2 (#4417): Rolldown's minifier (#4408) prints the same two
// compiler-owned sites as template literals, build:`<token>`.
describe("identity normalization v2 template-literal sites (L1)", () => {
  const backtick = (text) =>
    text.replaceAll('build:"' + REAL_TOKEN + '"', "build:`" + REAL_TOKEN + "`");
  const backtickEntry = () => {
    const entry = fixtureFiles("multi-island")[ENTRY].toString("latin1");
    expect(entry.split('build:"' + REAL_TOKEN + '"')).toHaveLength(3);
    return Buffer.from(backtick(entry), "latin1");
  };
  // A minimal entry with the registration site before the mount-local site.
  const sites = (registration, mount) =>
    Buffer.from(
      "function r(n){return{identity:{component:n,build:" +
        registration +
        "},mount:()=>{let l={component:n,build:" +
        mount +
        "};return l}}}\n",
      "latin1",
    );
  const tick = "`" + REAL_TOKEN + "`";

  it("states the v2 rules", () => {
    expect(IDENTITY_NORMALIZATION_VERSION).toBe(2);
    expect(IDENTITY_NORMALIZATION_RULES.startsWith("v2: ")).toBe(true);
    expect(IDENTITY_NORMALIZATION_RULES).toContain(
      'exact build:"<that token>" or build:`<that token>` occurrences (same quote on both sides)',
    );
    expect(IDENTITY_NORMALIZATION_RULES).toContain("require exactly 2 sites in the islands entry");
  });

  it("normalizes both backtick sites of an entry to the backtick placeholder", () => {
    const entry = backtickEntry();
    const { bytes, sites: found } = normalizeIdentitySites(entry, REAL_TOKEN, 2, ENTRY);
    expect(found).toBe(2);
    expect(bytes.length).toBe(entry.length);
    const text = bytes.toString("latin1");
    expect(text.includes(REAL_TOKEN)).toBe(false);
    expect(text.match(new RegExp("build:`" + IDENTITY_PLACEHOLDER + "`", "g"))).toHaveLength(2);
    expect(text).not.toContain('build:"' + IDENTITY_PLACEHOLDER);
    expect(text).toContain("identity:{component:n,build:`" + IDENTITY_PLACEHOLDER + "`}");
    expect(text).toContain("let l={component:n,build:`" + IDENTITY_PLACEHOLDER + "`");
    // Same bytes as the double-quoted v1 shape once the quotes are swapped back.
    const quoted = normalizeIdentitySites(
      fixtureFiles("multi-island")[ENTRY],
      REAL_TOKEN,
      2,
      ENTRY,
    );
    expect(
      text.replaceAll(
        "build:`" + IDENTITY_PLACEHOLDER + "`",
        'build:"' + IDENTITY_PLACEHOLDER + '"',
      ),
    ).toBe(quoted.bytes.toString("latin1"));
  });

  it("does not count a mixed-quote shape as a site", () => {
    for (const mixed of ['"' + REAL_TOKEN + "`", "`" + REAL_TOKEN + '"']) {
      const entry = sites(tick, mixed);
      expect(() => normalizeIdentitySites(entry, REAL_TOKEN, 2, ENTRY)).toThrow(
        "expected 2 build identity site(s), found 1",
      );
      const { bytes, sites: found } = normalizeIdentitySites(entry, REAL_TOKEN, 1, ENTRY);
      expect(found).toBe(1);
      const text = bytes.toString("latin1");
      expect(text).toContain("identity:{component:n,build:`" + IDENTITY_PLACEHOLDER + "`}");
      expect(text).toContain("let l={component:n,build:" + mixed + "}");
    }
    expect(normalizeIdentitySites(sites(tick, tick), REAL_TOKEN, 2, ENTRY).sites).toBe(2);
  });

  it("leaves backtick sites in comments and strings alone", () => {
    const entry = backtickEntry();
    const decoys = Buffer.from(
      ";/* identity:{component:n,build:" +
        tick +
        "} */// let y={component:n,build:" +
        tick +
        "}\nconst text='let x={component:n,build:" +
        tick +
        "}';const other=\"identity:{component:n,build:" +
        tick +
        '}";\n',
      "latin1",
    );
    const bytes = Buffer.concat([entry, decoys]);
    const normalized = normalizeIdentitySites(bytes, REAL_TOKEN, 2, ENTRY);
    expect(normalized.sites).toBe(2);
    expect(normalized.bytes.subarray(entry.length).equals(decoys)).toBe(true);
    expect(normalized.bytes.subarray(0, entry.length).toString("latin1")).not.toContain(REAL_TOKEN);
  });

  it("rejects a backtick site carrying another token", () => {
    const other = "0123456789abcdef";
    const entry = backtickEntry();
    const oneForeign = Buffer.from(
      entry.toString("latin1").replace(tick, "`" + other + "`"),
      "latin1",
    );
    expect(() => normalizeIdentitySites(oneForeign, REAL_TOKEN, 2, ENTRY)).toThrow(
      "build identity site carries 0123456789abcdef but index.html declares " + REAL_TOKEN,
    );
    expect(() =>
      normalizeIdentitySites(sites(tick, "`" + other + "`"), REAL_TOKEN, 2, ENTRY),
    ).toThrow("build identity site carries 0123456789abcdef");
  });
});

describe("identity normalization over the real v4.3.1 artifact bytes (L3)", () => {
  it("reproduces the v4.3.1 Linux workspace multi-island total from the fixture bytes", () => {
    const result = normalizeFixtureFiles(fixtureFiles("multi-island"), "multi-island");
    expect(result.shipped).toEqual({
      raw: V431_MULTI_ISLAND.raw,
      gzip: 22337,
      gzipNormalized: V431_MULTI_ISLAND.gzipNormalized,
    });
    expect(result.chunks[0].sha256).toBe(
      "dca258bafc72ce5b5d53c9aaf4e151b253b402805fe0013aca33f9b8fb4cff09",
    );
  });

  it("(a) keeps gzipNormalized identical when only the identity token changes", () => {
    const original = fixtureFiles("multi-island");
    const baseline = normalizeFixtureFiles(original, "multi-island");
    for (const token of ["0123456789abcdef", "fedcba9876543210", "a".repeat(16)]) {
      const moved = normalizeFixtureFiles(retoken(original, REAL_TOKEN, token), "multi-island");
      expect(moved.buildToken).toBe(token);
      expect(moved.shipped.raw).toBe(baseline.shipped.raw);
      expect(moved.shipped.gzipNormalized).toBe(baseline.shipped.gzipNormalized);
      expect(moved.chunks[0].sha256Normalized).toBe(baseline.chunks[0].sha256Normalized);
      expect(moved.chunks[0].sha256).not.toBe(baseline.chunks[0].sha256);
    }
    const drifted = normalizeFixtureFiles(
      retoken(original, REAL_TOKEN, "fedcba9876543210"),
      "multi-island",
    );
    // The actual gzip is free to move with the token; it is evidence only.
    expect(typeof drifted.shipped.gzip).toBe("number");
  });

  it("(b) still measures unrelated strings, code edits, and extra chunks", () => {
    const original = fixtureFiles("multi-island");
    const baseline = normalizeFixtureFiles(original, "multi-island");
    const ceiling = V431_MULTI_ISLAND;

    let seed = 0x3471b00b;
    const noise = Buffer.alloc(96);
    for (let index = 0; index < noise.length; index += 1) {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
      noise[index] = seed >>> 24;
    }
    const appended = clone(original);
    appended[ENTRY] = Buffer.concat([
      original[ENTRY],
      Buffer.from("\n//" + noise.toString("base64") + "\n"),
    ]);
    const grown = normalizeFixtureFiles(appended, "multi-island");
    expect(grown.shipped.raw).toBeGreaterThan(ceiling.raw);
    expect(grown.shipped.gzipNormalized).toBeGreaterThan(ceiling.gzipNormalized);

    const edited = clone(original);
    edited[ENTRY] = Buffer.from(
      original[ENTRY]
        .toString("latin1")
        .replace("[zfb] duplicate owned island marker: ", "[zfb] DUPLICATE owned island marker: "),
      "latin1",
    );
    const changed = normalizeFixtureFiles(edited, "multi-island");
    expect(changed.shipped.raw).toBe(baseline.shipped.raw);
    expect(changed.chunks[0].sha256Normalized).not.toBe(baseline.chunks[0].sha256Normalized);

    const withChunk = clone(original);
    withChunk["assets/islands-chunk-lazy.js"] = Buffer.from("export const lazy = 1;\n");
    const extra = normalizeFixtureFiles(withChunk, "multi-island");
    expect(extra.chunks.map((chunk) => chunk.role)).toEqual(["lazy", "entry"]);
    expect(extra.shipped.gzipNormalized).toBe(
      baseline.shipped.gzipNormalized + gzip(withChunk["assets/islands-chunk-lazy.js"]),
    );
    expect(extra.shipped.gzipNormalized).toBeGreaterThan(ceiling.gzipNormalized);
  });

  it("(c) rejects measurement.json claims that disagree with the saved bytes", () => {
    const cases = {
      "multi-island": fixtureFiles("multi-island"),
      "no-island": fixtureFiles("no-island"),
    };
    expect(normalize(cases).results["multi-island"].shipped.gzipNormalized).toBe(
      V431_MULTI_ISLAND.gzipNormalized,
    );
    for (const [field, tamper] of [
      ["gzip", (result) => (result.chunks[0].gzip -= 1)],
      ["raw", (result) => (result.chunks[0].raw += 1)],
      ["sha256", (result) => (result.chunks[0].sha256 = "0".repeat(64))],
    ]) {
      const { measurementText, artifacts } = measurementFor(cases);
      const measurement = JSON.parse(measurementText);
      tamper(measurement.results["multi-island"]);
      expect(() =>
        normalizeMeasurement({ measurementText: JSON.stringify(measurement), artifacts }),
      ).toThrow(new RegExp("measurement.json claims " + field));
    }
    const { measurementText, artifacts } = measurementFor(cases);
    const measurement = JSON.parse(measurementText);
    measurement.results["multi-island"].shipped.gzip += 1;
    expect(() =>
      normalizeMeasurement({ measurementText: JSON.stringify(measurement), artifacts }),
    ).toThrow(/measurement.json shipped totals .* disagree with the saved bytes/);

    const differingPasses = measurementFor(cases);
    differingPasses.artifacts["multi-island"].passes[2].files[ENTRY] = Buffer.concat([
      differingPasses.artifacts["multi-island"].passes[2].files[ENTRY],
      Buffer.from("\n"),
    ]);
    expect(() => normalizeMeasurement(differingPasses)).toThrow(
      "workspace multi-island: saved repeated dist inventories differ",
    );
  });

  it("(e) accounts entry, shared, and lazy roles from bytes and sums them", () => {
    const original = fixtureFiles("multi-island");
    const files = clone(original);
    files[ENTRY] = Buffer.concat([
      Buffer.from('import "./chunk-shared.js";\n'),
      original[ENTRY],
      Buffer.from('\nvoid import("./islands-chunk-lazy.js");\n'),
    ]);
    files["assets/chunk-shared.js"] = Buffer.from("export const shared = 'shared';\n");
    files["assets/islands-chunk-lazy.js"] = Buffer.from("export const lazy = 'lazy';\n");
    const result = normalizeFixtureFiles(files, "multi-island");
    expect(result.entryPaths).toEqual([ENTRY]);
    expect(
      result.chunks.map(({ file, phase, role, identitySites }) => [
        file,
        phase,
        role,
        identitySites,
      ]),
    ).toEqual([
      ["assets/chunk-shared.js", "initial", "shared", 0],
      ["assets/islands-chunk-lazy.js", "later", "lazy", 0],
      [ENTRY, "initial", "entry", 2],
    ]);
    const sum = (rows, key) => rows.reduce((total, chunk) => total + chunk[key], 0);
    for (const key of ["raw", "gzip", "gzipNormalized"]) {
      expect(result.initial[key]).toBe(
        sum(
          result.chunks.filter((c) => c.phase === "initial"),
          key,
        ),
      );
      expect(result.later[key]).toBe(
        sum(
          result.chunks.filter((c) => c.phase === "later"),
          key,
        ),
      );
      expect(result.shipped[key]).toBe(sum(result.chunks, key));
    }
    for (const chunk of result.chunks) {
      expect(chunk.raw).toBe(files[chunk.file].length);
      expect(chunk.gzip).toBe(gzip(files[chunk.file]));
      expect(chunk.sha256).toBe(sha(files[chunk.file]));
      if (chunk.role !== "entry") expect(chunk.gzipNormalized).toBe(chunk.gzip);
    }
    const entry = result.chunks.find((chunk) => chunk.role === "entry");
    expect(entry.gzipNormalized).not.toBe(entry.gzip);
  });

  it("writes a versioned sidecar with runner provenance from a saved measurement directory", () => {
    const root = writeMeasurementDirectory({
      "multi-island": fixtureFiles("multi-island"),
      "no-island": fixtureFiles("no-island"),
    });
    const { outDir, sidecar } = normalizeMeasurementDirectory(root);
    expect(outDir).toBe(root);
    expect(sidecar).toMatchObject({
      schemaVersion: 1,
      metric: IDENTITY_NORMALIZED_METRIC,
      normalizationVersion: IDENTITY_NORMALIZATION_VERSION,
      placeholder: IDENTITY_PLACEHOLDER,
      identitySitesByRole: { entry: 2, shared: 0, lazy: 0 },
      rules: IDENTITY_NORMALIZATION_RULES,
      compression: linuxContract.compression,
      source: {
        file: "measurement.json",
        sha256: sha(readFileSync(join(root, "measurement.json"))),
        mode: "workspace",
        nodeVersion: "v24.14.0",
        runnerSha256: linuxContract.runnerSha256,
      },
      normalizer: {
        file: NORMALIZER_FILE,
        sha256: normalizerSha256(),
        nodeVersion: process.version,
        zlibVersion: process.versions.zlib,
      },
    });
    expect(IDENTITY_NORMALIZED_METRIC).toBe("gzip-identity-normalized");
    expect(IDENTITY_NORMALIZATION_VERSION).toBe(2);
    expect(IDENTITY_NORMALIZATION_RULES.startsWith("v2:")).toBe(true);
    expect(sidecar.normalizer.sha256).toBe(normalizerSha256());
    expect(linuxContract.normalizerSha256).toBe(sidecar.normalizer.sha256);
    expect(sidecar.results["multi-island"]).toMatchObject({
      buildToken: REAL_TOKEN,
      shipped: {
        raw: 67072,
        gzip: 22337,
        gzipNormalized: V431_MULTI_ISLAND.gzipNormalized,
      },
    });
    expect(sidecar.results["no-island"].shipped).toEqual({ raw: 0, gzip: 0, gzipNormalized: 0 });
  });
});
