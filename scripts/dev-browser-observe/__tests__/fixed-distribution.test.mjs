import {
  chmodSync,
  cpSync,
  lstatSync,
  readdirSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, describe, expect, it, vi } from "vitest";
import { EventEmitter } from "node:events";
import * as safetyModule from "../lib/fixed-rss-safety.mjs";
import * as custodyModule from "../lib/fixed-rss-custody.mjs";
import * as familyModule from "../lib/fixed-family-admission.mjs";
import * as lockModule from "../lib/lock.mjs";
import { aggregate } from "../lib/aggregate.mjs";
import {
  appTreeHash,
  classifyRoles,
  compareLockfile,
  createReadiness,
  distributionChecks,
  distributionOf,
  installDistribution,
  launchCommand,
  loadDistributions,
  logicalText,
  materializeFixed,
  nodeIdentity,
  processExecutable,
  processIdentityChecks,
  verifyInstall,
} from "../lib/distribution.mjs";
import {
  loadFieldNotesFixture,
  loadFieldNotesScales,
  planFieldNotesOp,
  sourceTreeHashes,
} from "../lib/fieldnotes.mjs";
import { fixedIdentityVerdict } from "../lib/fixed.mjs";
import { freezeHarness, unfreezeForRemoval } from "../lib/frozen.mjs";
import { currentPins } from "../lib/identity.mjs";
import { OBSERVER_PATH } from "../lib/browser.mjs";
import { profileSchemaFor, semanticErrors, loadProfile } from "../lib/profile.mjs";
import { stopOwned, snapshotOwnedWithArgs } from "../lib/process-owner.mjs";
import { execute } from "./isolated-session.mjs";
import { EXIT } from "../lib/run.mjs";
import { validateAs } from "../lib/schema.mjs";
import { loadSeriesPlan } from "../lib/series.mjs";
import { startDevServer } from "../lib/server.mjs";
import { HARNESS_DIR, ensureOwnerWritable, readJson, sha256, sha256File } from "../lib/util.mjs";
import { SPEC, makeFakeSource, makeTinyHarness } from "./fieldnotes-fake.mjs";
import { fakePage } from "./fakes.mjs";

const FIXTURE = join(HARNESS_DIR, "fixtures", "field-notes");
const example = (n) => JSON.parse(readFileSync(join(HARNESS_DIR, "profiles", n), "utf8"));
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-fixed-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) rmSync(tmps.pop(), { recursive: true, force: true });
});

// One read-only base (fake source, fixture, three installs) for the tests that never mutate it,
// so the file creates far fewer files; tests that tamper build their own with setup().
let sharedBase = null;
const sharedRoots = [];
function shared() {
  if (!sharedBase) {
    const s = setup({ keep: sharedRoots });
    const installs = Object.fromEntries(["zfb", "astro", "next"].map((fw) => [fw, install(s, fw)]));
    sharedBase = { s, installs };
  }
  return sharedBase;
}
afterAll(() => {
  while (sharedRoots.length) rmSync(sharedRoots.pop(), { recursive: true, force: true });
});

const FOOTER = "<footer><p>Same content. Same design. Three build systems.</p></footer>\n";

/** Per-framework fake app layout: the files the declarations name, with tiny bytes. */
const FAKE = {
  zfb: {
    pinned: { "@takazudo/zfb": "6.0.1", "@takazudo/zfb-runtime": "6.0.1" },
    files: {
      "lib/spec.json": JSON.stringify(SPEC),
      "lib/content.ts": "// archive helper replaced by the adapter\n",
      "components/Layout.tsx": FOOTER,
      "public/site.css": ":root{color-scheme:light}\n",
      "zfb.config.ts": "export default {};\n",
    },
    bin: ["@takazudo/zfb", "bin/zfb.mjs"],
  },
  astro: {
    pinned: { astro: "7.4.12", vite: "8.3.8" },
    files: {
      "spec.json": JSON.stringify(SPEC),
      "src/lib/content.js": "export const getIndex = () => null;\n",
      "src/layouts/Site.astro": FOOTER,
      "public/site.css": ":root{color-scheme:light}\n",
      "astro.config.mjs": "export default {};\n",
    },
    bin: ["astro", "bin/astro.mjs"],
  },
  next: {
    pinned: { next: "16.4.6", react: "19.3.1", "react-dom": "19.3.1" },
    files: {
      "spec.json": JSON.stringify(SPEC),
      "lib/content.js": "export const getIndex = () => null;\n",
      "lib/content-revision.js": "export const contentRevision = 'x:0';\n",
      "scripts/dev-content.mjs": "// sidecar\n",
      "app/layout.jsx": FOOTER,
      "public/site.css": ":root{color-scheme:light}\n",
      "next.config.mjs": "export default {};\n",
    },
    bin: ["next", "dist/bin/next"],
  },
};

function writeTree(root, files) {
  for (const [rel, text] of Object.entries(files)) {
    mkdirSync(join(root, rel, ".."), { recursive: true });
    writeFileSync(join(root, rel), text);
  }
}

/** package.json + package-lock.json (v3) for the pinned packages plus one optional platform package. */
function writeLock(appDir, pinned) {
  const packages = { "": { name: "fake", dependencies: pinned } };
  for (const [name, version] of Object.entries(pinned))
    packages[`node_modules/${name}`] = { version, integrity: `sha512-${sha256(name)}` };
  packages["node_modules/@fake/platform-other-os"] = {
    version: "1.0.0",
    integrity: "sha512-opt",
    optional: true,
  };
  writeFileSync(
    join(appDir, "package.json"),
    JSON.stringify({ name: "fake", dependencies: pinned }),
  );
  writeFileSync(
    join(appDir, "package-lock.json"),
    JSON.stringify({ lockfileVersion: 3, packages }),
  );
  return packages;
}

/** What `npm ci` would leave: package dirs, bin scripts, a .bin link and the hidden lockfile. */
function fakeNpmCi(appDir, framework) {
  const lock = readJson(join(appDir, "package-lock.json"));
  const hidden = { lockfileVersion: 3, packages: {} };
  for (const [key, e] of Object.entries(lock.packages)) {
    if (key === "" || e.optional) continue;
    hidden.packages[key] = { version: e.version, integrity: e.integrity };
    mkdirSync(join(appDir, key), { recursive: true });
    writeFileSync(join(appDir, key, "package.json"), JSON.stringify({ version: e.version }));
  }
  const [pkg, bin] = FAKE[framework].bin;
  writeTree(join(appDir, "node_modules", pkg), { [bin]: "#!/usr/bin/env node\n" });
  if (framework === "zfb") {
    writeTree(join(appDir, "node_modules", "@takazudo", "zfb-darwin-arm64"), { zfb: "native" });
    writeTree(join(appDir, "node_modules", "@takazudo", "zfb-linux-x64-gnu"), { zfb: "native" });
    writeTree(join(appDir, "node_modules", "@takazudo", "zfb-darwin-x64"), { zfb: "native" });
    writeTree(join(appDir, "node_modules", "@takazudo", "zfb-linux-arm64-gnu"), { zfb: "native" });
  }
  mkdirSync(join(appDir, "node_modules", ".bin"), { recursive: true });
  symlinkSync(`../${pkg}/${bin}`, join(appDir, "node_modules", ".bin", framework));
  writeFileSync(join(appDir, "node_modules", ".package-lock.json"), JSON.stringify(hidden));
}

/**
 * A temp fixture whose distributions.json and scales.json describe fakes: a 40-article source
 * (scale 100 declaration rewritten for it) with apps/<fw> for all three frameworks.
 */
function setup({ keep = null } = {}) {
  let root;
  if (keep) {
    root = mkdtempSync(join(tmpdir(), "dbo-fixed-shared-"));
    keep.push(root);
  } else root = tmp();
  const src = join(root, "src");
  const fake = makeFakeSource(src, 20);
  for (const fw of ["astro", "next"]) writeTree(join(src, "apps", fw), FAKE[fw].files);
  writeTree(join(src, "apps", "zfb"), FAKE.zfb.files);
  for (const fw of Object.keys(FAKE)) writeLock(join(src, "apps", fw), FAKE[fw].pinned);
  const fixtureDir = join(root, "fixture");
  cpSync(FIXTURE, fixtureDir, { recursive: true });
  const manifest = loadFieldNotesFixture(fixtureDir);
  const scales = loadFieldNotesScales(fixtureDir, manifest);
  const hashes = sourceTreeHashes(src, manifest);
  scales.byCount["100"] = {
    ...scales.byCount["100"],
    ...hashes,
    articleCount: 20,
    contentManifestSha256: sha256File(join(src, "content", "manifest.json")),
  };
  writeFileSync(join(fixtureDir, manifest.scales), JSON.stringify(scales));
  const dists = loadDistributions(fixtureDir, manifest);
  for (const fw of Object.keys(FAKE)) {
    const e = dists.frameworks[fw];
    const app = join(src, e.app);
    e.appTree.sha256 = appTreeHash(app, e).sha256;
    e.packageJsonSha256 = sha256File(join(app, "package.json"));
    e.lockfile.sha256 = sha256File(join(app, "package-lock.json"));
    e.pinned = FAKE[fw].pinned;
    e.config.sha256 = sha256File(join(app, e.config.path));
    if (e.helper.sha256) e.helper.sha256 = sha256File(join(app, e.helper.path));
    for (const f of e.adapterFiles) if (!f.volatile) f.sha256 = sha256File(join(app, f.path));
    for (const x of e.executables) delete x.sha256;
  }
  writeFileSync(join(fixtureDir, manifest.distributions), JSON.stringify(dists));
  return { root, src, fixtureDir, manifest, hashes, fake };
}

function install(s, framework, { onCi = () => {} } = {}) {
  const out = join(s.root, `install-${framework}`);
  const r = installDistribution({
    fixtureDir: s.fixtureDir,
    framework,
    sourceDir: s.src,
    outDir: out,
    npmCli: "/fake/npm-cli.js",
    spawnImpl: (_cmd, _args, opts) => {
      fakeNpmCi(opts.cwd, framework);
      onCi(opts.cwd);
      return { status: 0, stdout: "added 3 packages", stderr: "" };
    },
  });
  return { out, receipt: r, entry: distributionOf(s.fixtureDir, framework) };
}

function fixedProfile(s, framework, installDir, extra = {}, name = `${framework}.json`) {
  const p = example(
    framework === "zfb"
      ? "field-notes-fixed-zfb-original-memoized-100.example.json"
      : `field-notes-fixed-${framework}-100.example.json`,
  );
  p.fixture.dir = s.fixtureDir;
  p.fixture.source = { dir: s.src, ...s.hashes };
  p.distribution.installDir = installDir;
  p.outDir = join(s.root, "out");
  const entry = distributionOf(s.fixtureDir, framework);
  const helper =
    framework === "zfb"
      ? join(s.fixtureDir, s.manifest.adapters.variants[p.fixture.adapter].file)
      : join(s.src, entry.app, entry.helper.path);
  p.pins = currentPins({
    fixtureDir: s.fixtureDir,
    observerPath: OBSERVER_PATH,
    adapterPath: helper,
  });
  Object.assign(p, extra);
  const path = join(s.root, name);
  writeFileSync(path, JSON.stringify(p));
  return { path, profile: p };
}

const verdictOf = (path) => {
  const { profile, resolved } = loadProfile(path);
  return fixedIdentityVerdict({
    profile,
    resolved,
    manifest: loadFieldNotesFixture(resolved.fixtureDir),
  });
};
const failed = (v) => v.checks.filter((c) => !c.ok).map((c) => c.name);

describe("declarations", () => {
  it("distributions.json validates; every imported-CSS file is the archive bytes it names", () => {
    const manifest = loadFieldNotesFixture(FIXTURE);
    const d = loadDistributions(FIXTURE, manifest);
    expect(validateAs("field-notes-distributions", d)).toEqual([]);
    expect(d.frameworks.zfb.pinned).toEqual({
      "@takazudo/zfb": "6.0.1",
      "@takazudo/zfb-runtime": "6.0.1",
    });
    expect(d.frameworks.astro.pinned.astro).toBe("7.4.12");
    expect(d.frameworks.next.pinned).toEqual({
      next: "16.4.6",
      react: "19.3.1",
      "react-dom": "19.3.1",
    });
    for (const fw of ["zfb", "astro", "next"])
      for (const f of d.frameworks[fw].importedCss.files)
        expect(sha256File(join(FIXTURE, f.file)), f.file).toBe(f.sha256);
    expect(d.frameworks.zfb.appTree.sha256).toBe(
      loadFieldNotesScales(FIXTURE, manifest).byCount["100"].appsZfbTreeSha256,
    );
  });
});

describe("mode-aware profile schema", () => {
  it("fixed-distribution examples validate against their own schema; source-built is unchanged", () => {
    for (const n of [
      "field-notes-fixed-zfb-original-memoized-100.example.json",
      "field-notes-fixed-zfb-recovered-snapshot-100.example.json",
      "field-notes-fixed-astro-100.example.json",
      "field-notes-fixed-next-100.example.json",
    ]) {
      const p = example(n);
      expect(profileSchemaFor(p)).toBe("profile-fixed-distribution");
      expect(validateAs("profile-fixed-distribution", p), n).toEqual([]);
      expect(semanticErrors(p), n).toEqual([]);
    }
    const sb = example("field-notes-direct-read.example.json");
    expect(profileSchemaFor(sb)).toBe("profile");
    const { productSha: _p, ...noSha } = sb;
    expect(validateAs("profile", noSha)).toContain("$: missing productSha");
    expect(validateAs("profile", { ...sb, mode: "fixed-distribution" })[0]).toMatch(/not in/);
  });

  it("fixed profiles need framework and installDir, carry no build receipt, and use their framework's helper", () => {
    const p = example("field-notes-fixed-astro-100.example.json");
    const { framework: _f, ...noFw } = p;
    expect(validateAs("profile-fixed-distribution", noFw)).toContain("$: missing framework");
    expect(validateAs("profile-fixed-distribution", { ...p, distribution: {} })).toContain(
      "$.distribution: missing installDir",
    );
    expect(
      validateAs("profile-fixed-distribution", { ...p, productSha: "a".repeat(40) }),
    ).toContain("$: unexpected property productSha");
    expect(validateAs("profile-fixed-distribution", { ...p, diagnostic: true })[0]).toMatch(
      /expected false/,
    );
    expect(
      semanticErrors({ ...p, fixture: { ...p.fixture, adapter: "original-memoized" } }).join("\n"),
    ).toMatch(/astro runs archive/);
    const z = example("field-notes-fixed-zfb-original-memoized-100.example.json");
    expect(
      semanticErrors({ ...z, fixture: { ...z.fixture, adapter: "direct-read" } }).join("\n"),
    ).toMatch(/zfb runs original-memoized or recovered-snapshot/);
    expect(
      semanticErrors({ ...z, distribution: { ...z.distribution, launch: "npm-script" } }).join(
        "\n",
      ),
    ).toMatch(/no dev script/);
    expect(
      semanticErrors({ ...p, fixture: { ...p.fixture, importedCss: false } }).join("\n"),
    ).toMatch(/edit-managed-css needs fixture.importedCss: true/);
    expect(
      semanticErrors({ ...p, scenarios: [{ ...p.scenarios[0], set: "recovery" }] }).join("\n"),
    ).toMatch(/source-built only/);
  });
});

describe("install verification against the archive lockfile", () => {
  it("accepts an npm ci result and records versions, executables and the node_modules tree", () => {
    const { installs } = shared();
    for (const fw of ["zfb", "astro", "next"]) {
      const { receipt } = installs[fw];
      expect(receipt.ok, fw).toBe(true);
      expect(validateAs("install-receipt", receipt)).toEqual([]);
      expect(receipt.command.slice(1)).toEqual([
        "/fake/npm-cli.js",
        "ci",
        "--no-audit",
        "--no-fund",
      ]);
      expect(receipt.verification.facts.lockComparison.skippedOptional).toEqual([
        "node_modules/@fake/platform-other-os",
      ]);
      expect(receipt.verification.facts.nodeModules.symlinks).toBe(1);
      expect(receipt.node.version).toBe(process.version);
    }
  });

  it("rejects a wrong version, a tampered lockfile, an integrity mismatch, extraneous or missing packages and caches", () => {
    const s = setup();
    const { out, entry } = install(s, "astro");
    const app = join(out, entry.app);
    const names = () => failed(verifyInstall({ installDir: out, entry }));
    writeFileSync(
      join(app, "node_modules/astro/package.json"),
      JSON.stringify({ version: "7.4.13" }),
    );
    expect(names()).toEqual(["pinned:astro", "lockfile-installed-match"]);
    writeFileSync(
      join(app, "node_modules/astro/package.json"),
      JSON.stringify({ version: "7.4.12" }),
    );
    const hidden = readJson(join(app, "node_modules/.package-lock.json"));
    hidden.packages["node_modules/vite"].integrity = "sha512-other";
    hidden.packages["node_modules/left-pad"] = { version: "1.0.0" };
    writeFileSync(join(app, "node_modules/.package-lock.json"), JSON.stringify(hidden));
    expect(compareLockfile(app).problems.map((p) => `${p.kind}:${p.key}`)).toEqual([
      "integrity:node_modules/vite",
      "extraneous:node_modules/left-pad",
    ]);
    delete hidden.packages["node_modules/vite"];
    delete hidden.packages["node_modules/left-pad"];
    writeFileSync(join(app, "node_modules/.package-lock.json"), JSON.stringify(hidden));
    expect(compareLockfile(app).problems.map((p) => p.kind)).toEqual(["missing"]);
    writeFileSync(
      join(app, "package-lock.json"),
      `${readFileSync(join(app, "package-lock.json"))} `,
    );
    mkdirSync(join(app, ".astro"));
    expect(names()).toEqual(
      expect.arrayContaining(["lockfile", "lockfile-installed-match", "no-framework-caches"]),
    );
  });

  it("an npm failure records the failure and no verification; a wrong source app copies nothing", () => {
    const s = setup();
    const out = join(s.root, "install-fail");
    const r = installDistribution({
      fixtureDir: s.fixtureDir,
      framework: "next",
      sourceDir: s.src,
      outDir: out,
      npmCli: "/fake/npm-cli.js",
      spawnImpl: () => ({ status: 1, stdout: "", stderr: "npm ERR! network" }),
    });
    expect(r).toMatchObject({ ok: false, exitCode: 1, verification: null });
    expect(r.stderrTail).toMatch(/network/);
    writeFileSync(join(s.src, "apps", "next", "lib", "content.js"), "// different helper\n");
    expect(() =>
      installDistribution({
        fixtureDir: s.fixtureDir,
        framework: "next",
        sourceDir: s.src,
        outDir: join(s.root, "never"),
        npmCli: "/fake/npm-cli.js",
        spawnImpl: () => {
          throw new Error("must not run");
        },
      }),
    ).toThrow(/is not the declared .*nothing copied/);
    expect(existsSync(join(s.root, "never"))).toBe(false);
    const plan = installDistribution({
      fixtureDir: s.fixtureDir,
      framework: "astro",
      sourceDir: s.src,
      outDir: join(s.root, "plan"),
      npmCli: "/x/npm-cli.js",
      dryRun: true,
    });
    expect(plan.plan.command.slice(1)).toEqual(["/x/npm-cli.js", "ci", "--no-audit", "--no-fund"]);
    expect(existsSync(join(s.root, "plan"))).toBe(false);
  });

  it("a different node binary or node_modules changed after install fails the distribution checks", () => {
    const s = setup();
    const { out, entry } = install(s, "zfb");
    expect(failed(distributionChecks({ installDir: out, entry }))).toEqual([]);
    const r = readJson(join(out, "install-receipt.json"));
    r.node.version = "v0.0.0";
    writeFileSync(join(out, "install-receipt.json"), JSON.stringify(r));
    expect(failed(distributionChecks({ installDir: out, entry }))).toEqual([
      "node-version-matches-install",
    ]);
    r.node.version = process.version;
    writeFileSync(join(out, "install-receipt.json"), JSON.stringify(r));
    writeFileSync(join(out, "apps/zfb/node_modules/@takazudo/zfb/bin/zfb.mjs"), "// patched\n");
    expect(failed(distributionChecks({ installDir: out, entry }))).toEqual([
      "node-modules-unchanged-since-install",
    ]);
  });
});

describe("fixed identity before any edit, per framework", () => {
  for (const fw of ["zfb", "astro", "next"])
    it(`${fw}: accepts the declared install, corpus, helper and pins`, () => {
      const { s, installs } = shared();
      const { path } = fixedProfile(s, fw, installs[fw].out, {}, `accept-${fw}.json`);
      const v = verdictOf(path);
      expect(failed(v)).toEqual([]);
      expect(v.facts.framework).toBe(fw);
      expect(v.facts.install.pinned.every((p) => p.ok)).toBe(true);
    });

  it("rejects a wrong scale and a wrong helper", () => {
    const s = setup();
    const { out } = install(s, "astro");
    const { path, profile } = fixedProfile(s, "astro", out);
    writeFileSync(
      path,
      JSON.stringify({ ...profile, fixture: { ...profile.fixture, scale: 5000 } }),
    );
    expect(failed(verdictOf(path))).toEqual(
      expect.arrayContaining([
        "source-dataset-matches-manifest-declaration",
        "source-article-count-matches-scale",
      ]),
    );
    writeFileSync(path, JSON.stringify(profile));
    writeFileSync(join(s.src, "apps", "astro", "src", "lib", "content.js"), "// modified helper\n");
    expect(failed(verdictOf(path))).toEqual(
      expect.arrayContaining(["source-app-tree", "helper:archive", "pin:adapterSha256"]),
    );
  });

  it("rejects a wrong lockfile in the install and a missing install dir", () => {
    const s2 = setup();
    const i2 = install(s2, "next");
    const p2 = fixedProfile(s2, "next", i2.out);
    writeFileSync(join(i2.out, "apps/next/package-lock.json"), "{}");
    expect(failed(verdictOf(p2.path))).toEqual(
      expect.arrayContaining(["install:lockfile", "install:lockfile-installed-match"]),
    );
    const p3 = fixedProfile(s2, "next", join(s2.root, "nowhere"));
    expect(failed(verdictOf(p3.path))).toContain("install-dir");
  });

  it("execute refuses a mismatched fixed install before creating the work dir", async () => {
    const s = setup();
    const { out } = install(s, "zfb");
    const { path } = fixedProfile(s, "zfb", out);
    writeFileSync(
      join(out, "apps/zfb/node_modules/@takazudo/zfb/package.json"),
      '{"version":"6.0.2"}',
    );
    const code = await execute({
      profilePath: path,
      label: "F1",
      command: "run",
      stderr: () => {},
    });
    expect(code).toBe(EXIT.identityRejected);
    expect(existsSync(join(s.root, "out", "F1", "work"))).toBe(false);
    const receipt = readJson(join(s.root, "out", "F1", "receipt.json"));
    expect(receipt).toMatchObject({
      mode: "fixed-distribution",
      framework: "zfb",
      buildReceipt: null,
    });
    expect(receipt.identity.checks.filter((c) => !c.ok).map((c) => c.name)).toEqual(
      expect.arrayContaining([
        "install:pinned:@takazudo/zfb",
        "node-modules-unchanged-since-install",
      ]),
    );
  });
});

describe("a fixed run end to end with a fake server and browser", () => {
  it("materializes, launches the framework command, records roles, runs the batch and records the receipt", async () => {
    const { s, installs } = shared();
    const { path, profile } = fixedProfile(s, "next", installs.next.out, {}, "e2e-next.json");
    profile.scenarios = [
      {
        id: "body",
        set: "workload",
        warmOrder: "none",
        ops: [
          {
            id: "b",
            op: "edit-body",
            article: "article-00005",
            writeMethod: "in-place",
            repeat: { warmups: 1, measured: 2 },
          },
        ],
      },
    ];
    writeFileSync(path, JSON.stringify(profile));
    const started = [];
    let resolveExit;
    const handle = {
      native: { pid: 100, pgid: 100, lstart: "x", comm: "node" },
      launcher: "none",
      exit: null,
      known: new Map(),
    };
    handle.exited = new Promise((r) => {
      resolveExit = r;
    });
    const server = (opts) => {
      started.push(opts);
      return {
        handle,
        args: opts.args,
        envOverrides: opts.env,
        zfbEnv: {},
        removedDiagnosticEnv: [],
        diagnostic: false,
        startedNs: process.hrtime.bigint(),
        ready: Promise.resolve({
          baseUrl: "http://127.0.0.1:4555",
          readyNs: process.hrtime.bigint(),
        }),
        closeLog: async () => ({}),
        isAlive: () => ({ alive: true, exit: null }),
        pid: () => 100,
        waitExit: () => handle.exited,
        sampleResources: () => ({ summedRssKiB: 1 }),
        sampleResourcesAsync: async () => ({ summedRssKiB: 1 }),
        logMark: () => 0,
        logSince: () => ({ lineCount: 0, lines: [] }),
      };
    };
    const page = fakePage({
      onWait: () =>
        Promise.resolve({
          kind: "notify",
          payload: { docUUID: "doc-1" },
          t1: process.hrtime.bigint() + 1_000_000n,
        }),
    });
    const code = await execute({
      profilePath: path,
      label: "E1",
      command: "run",
      stderr: () => {},
      hooks: {
        freePort: async () => 4555,
        startDevServer: server,
        snapshotOwned: () => [
          {
            pid: 100,
            ppid: 1,
            comm: "node",
            args: "node /w/apps/next/scripts/dev-content.mjs --port 4555",
          },
          {
            pid: 101,
            ppid: 100,
            comm: "node",
            args: "node /w/node_modules/next/dist/bin/next dev",
          },
          { pid: 102, ppid: 101, comm: "node", args: "next-server (v16.4.6)" },
        ],
        processExecutable: () => ({ path: process.execPath, method: "fake" }),
        processAlive: () => true,
        launchOwnedBrowser: async () => ({ observer: page, identity: { engine: "fake" } }),
        closeOwnedBrowser: async () => [],
        stopOwned: async () => {
          resolveExit((handle.exit = { exitCode: 0, signalCode: null }));
          return { absent: true };
        },
      },
    });
    expect(code).toBe(EXIT.ok);
    expect(started[0].args.slice(-4)).toEqual(["--hostname", "127.0.0.1", "--port", "4555"]);
    expect(started[0].args[0]).toMatch(/E1\/work\/apps\/next\/scripts\/dev-content\.mjs$/);
    expect(started[0].command).toBe(process.execPath);
    const dir = join(s.root, "out", "E1");
    const receipt = readJson(join(dir, "receipt.json"));
    expect(receipt.server.launch).toMatchObject({ kind: "direct", port: 4555 });
    expect(receipt.server.roles.map((r) => r.role)).toEqual([
      "content-sidecar",
      "next-cli",
      "next-server",
    ]);
    expect(receipt.server.rootRole).toBe("content-sidecar");
    expect(receipt.fixture).toMatchObject({
      mode: "fixed-distribution",
      framework: "next",
      adapter: "archive",
    });
    expect(receipt.fixture.materializedChecks.every((c) => c.ok)).toBe(true);
    const summary = readJson(join(dir, "summary.json"));
    expect(summary.accounting).toMatchObject({
      planned: 3,
      attempted: 3,
      success: 3,
      notAttempted: 0,
    });
    expect(summary.latencyConditionalOnSuccessMs.n).toBe(2);
    expect(readFileSync(join(dir, "work/content/articles/article-00005.md"), "utf8")).toBe(
      readFileSync(join(s.src, "content/articles/article-00005.md"), "utf8"),
    );
  });
});

describe("materialization and op mapping", () => {
  it("copies the install's app with node_modules, installs the zfb helper and the imported-CSS variant", () => {
    const { s, installs } = shared();
    const { out } = installs.zfb;
    const m = materializeFixed({
      fixtureDir: s.fixtureDir,
      sourceDir: s.src,
      installDir: out,
      workDir: join(s.root, "work"),
      framework: "zfb",
      adapter: "original-memoized",
      importedCss: true,
      scale: 100,
    });
    expect(m.helperSha256).toBe(s.manifest.adapters.variants["original-memoized"].sha256);
    expect(m.importedCss.map((f) => f.target)).toEqual([
      "components/Layout.tsx",
      "styles/global.css",
    ]);
    expect(m.managedCssPath).toBe(join(m.appDir, "styles/global.css"));
    expect(m.nodeModules.sha256).toBe(
      readJson(join(out, "install-receipt.json")).verification.facts.nodeModules.sha256,
    );
    expect(m.model.articles).toHaveLength(20);
    const a = planFieldNotesOp(m, m.model, { op: "edit-managed-css" }, { token: "tok-css" });
    expect(a.predicate).toEqual({
      kind: "computed-style",
      selector: ":root",
      property: "--fn-managed-marker",
      includes: "tok-css",
    });
    expect(a.writes[0].bytes.toString()).toMatch(/^:root\{--fn-managed-marker:tok-css;/);
    const f = planFieldNotesOp(m, m.model, { op: "edit-shared-component" }, { token: "tok-f" });
    expect(f.writes[0].path).toBe(join(m.appDir, "components/Layout.tsx"));
    expect(f.writes[0].bytes.toString()).toContain("Three build systems. tok-f");
  });

  for (const fw of ["astro", "next"])
    it(`${fw} keeps its archive helper; a zfb adapter name is refused`, () => {
      const { s, installs } = shared();
      {
        const { out, entry } = installs[fw];
        const m = materializeFixed({
          fixtureDir: s.fixtureDir,
          sourceDir: s.src,
          installDir: out,
          workDir: join(s.root, `work-${fw}`),
          framework: fw,
          adapter: "archive",
          importedCss: false,
          scale: 100,
        });
        expect(m.helperSha256).toBe(entry.helper.sha256);
        expect(m.layoutPath).toBe(join(m.appDir, entry.layout));
        expect(m.managedCssPath).toBeNull();
        expect(() =>
          materializeFixed({
            fixtureDir: s.fixtureDir,
            sourceDir: s.src,
            installDir: out,
            workDir: join(s.root, `work2-${fw}`),
            framework: fw,
            adapter: "original-memoized",
            scale: 100,
          }),
        ).toThrow(/runs only its archive helper/);
      }
    });
});

describe("fixed materialization from read-only sources", () => {
  it("a read-only scale source and install still give an owner-writable work copy with the same node_modules", () => {
    const s = setup();
    const { out, receipt } = install(s, "zfb");
    const readOnly = (dir) => {
      const st = lstatSync(dir);
      if (st.isSymbolicLink()) return;
      if (st.isDirectory()) {
        for (const n of readdirSync(dir)) readOnly(join(dir, n));
        chmodSync(dir, 0o555);
      } else chmodSync(dir, st.mode & 0o555);
    };
    readOnly(s.src);
    readOnly(out);
    try {
      const m = materializeFixed({
        fixtureDir: s.fixtureDir,
        sourceDir: s.src,
        installDir: out,
        workDir: join(s.root, "work-ro"),
        framework: "zfb",
        adapter: "original-memoized",
        importedCss: true,
        scale: 100,
      });
      expect(m.nodeModules.sha256).toBe(receipt.verification.facts.nodeModules.sha256);
      const notWritable = [];
      const visit = (p) => {
        const st = lstatSync(p);
        if (st.isSymbolicLink()) return;
        const want = st.isDirectory() ? 0o700 : 0o600;
        if ((st.mode & want) !== want) notWritable.push(p);
        if (st.isDirectory()) for (const n of readdirSync(p)) visit(join(p, n));
      };
      visit(m.workDir);
      expect(notWritable).toEqual([]);
      writeFileSync(join(m.contentDir, "article-00005.md"), "edited\n");
      writeFileSync(m.managedCssPath, ":root{}\n");
      expect(lstatSync(join(out, "apps/zfb/package.json")).mode & 0o222).toBe(0);
    } finally {
      ensureOwnerWritable(s.src);
      ensureOwnerWritable(out);
    }
  });
});

describe("launch commands and readiness from real archive output", () => {
  const entries = Object.fromEntries(
    ["zfb", "astro", "next"].map((fw) => [fw, distributionOf(FIXTURE, fw)]),
  );

  it("direct and npm-script commands, all with this node", () => {
    const base = { appDir: "/w/app", host: "127.0.0.1", port: 4400, nodePath: "/n/node" };
    expect(launchCommand({ ...base, entry: entries.zfb })).toEqual({
      kind: "direct",
      command: "/n/node",
      args: [
        "/w/app/node_modules/@takazudo/zfb/bin/zfb.mjs",
        "dev",
        "--host",
        "127.0.0.1",
        "--port",
        "4400",
      ],
    });
    expect(launchCommand({ ...base, entry: entries.astro }).args).toEqual([
      "/w/app/node_modules/astro/bin/astro.mjs",
      "dev",
      "--ignore-lock",
      "--host",
      "127.0.0.1",
      "--port",
      "4400",
    ]);
    expect(launchCommand({ ...base, entry: entries.next }).args).toEqual([
      "/w/app/scripts/dev-content.mjs",
      "--hostname",
      "127.0.0.1",
      "--port",
      "4400",
    ]);
    expect(
      launchCommand({ ...base, entry: entries.next, launch: "npm-script", npmCli: "/n/npm-cli.js" })
        .args,
    ).toEqual(["/n/npm-cli.js", "run", "dev", "--", "--hostname", "127.0.0.1", "--port", "4400"]);
    expect(() =>
      launchCommand({ ...base, entry: entries.zfb, launch: "npm-script", npmCli: "x" }),
    ).toThrow(/no npm dev script/);
  });

  it("zfb: the 6.0.1 ready line", () => {
    const r = createReadiness(entries.zfb.readiness, { port: 4311 });
    expect(r.push("info listening on 127.0.0.1:4311 (routes pending)")).toBeNull();
    expect(r.push("→ ready on http://127.0.0.1:4311")).toMatchObject({
      url: "http://127.0.0.1:4311",
    });
  });

  it("astro: the JSON log message with version and Local URL", () => {
    const line =
      '{"message":" astro  v7.4.12 ready in 2092 ms\\n┃ Local    http://127.0.0.1:4312/","label":"SKIP_FORMAT","level":"info"}';
    expect(logicalText(line)).toContain("┃ Local");
    const r = createReadiness(entries.astro.readiness, { port: 4312 });
    expect(r.push('{"message":"Synced content","label":"content","level":"info"}')).toBeNull();
    expect(r.push(line)).toMatchObject({ url: "http://127.0.0.1:4312" });
    const wrong = createReadiness(entries.astro.readiness, { port: 4312 });
    expect(wrong.push(line.replace("v7.4.12", "v7.5.0"))).toBeNull();
  });

  it("next: sidecar, version and Ready are all required; a different port is a failure", () => {
    const lines = [
      "[field-notes-dev] external content watcher ready (100 files; 20 ms debounce)",
      "▲ Next.js 16.4.6 (Turbopack)",
      "- Local:         http://127.0.0.1:4313",
      "- Network:       http://127.0.0.1:4313",
      "✓ Ready in 622ms",
    ];
    const r = createReadiness(entries.next.readiness, { port: 4313 });
    const results = lines.map((l) => r.push(l));
    expect(results.slice(0, 4)).toEqual([null, null, null, null]);
    expect(results[4]).toMatchObject({ url: "http://127.0.0.1:4313" });
    const noSidecar = createReadiness(entries.next.readiness, { port: 4313 });
    expect(
      lines
        .slice(1)
        .map((l) => noSidecar.push(l))
        .at(-1),
    ).toBeNull();
    const moved = createReadiness(entries.next.readiness, { port: 9999 });
    expect(moved.push(lines[2])).toEqual({
      failed: { reason: "ready-port-mismatch", expected: "9999", printed: "http://127.0.0.1:4313" },
    });
  });

  it("startDevServer resolves from the framework readiness and rejects a contradicted one", async () => {
    const root = tmp();
    const fakeSpawn =
      (lines) =>
      (cmd, args, { onStdout }) => {
        setTimeout(() => onStdout(Buffer.from(`${lines.join("\n")}\n`)), 1);
        return {
          native: { pid: 1, pgid: 1 },
          exit: null,
          exited: new Promise(() => {}),
          child: {},
        };
      };
    const ok = startDevServer({
      command: "/n/node",
      args: ["x"],
      readiness: createReadiness(entries.next.readiness, { port: 4313 }),
      appDir: root,
      port: { mode: "fixed", port: 4313 },
      readyTimeoutMs: 2000,
      logPath: join(root, "a.log"),
      spawnImpl: fakeSpawn([
        "[field-notes-dev] external content watcher ready (100 files; 20 ms debounce)",
        "▲ Next.js 16.4.6 (Turbopack)",
        "- Local:         http://127.0.0.1:4313",
        "✓ Ready in 622ms",
      ]),
    });
    await expect(ok.ready).resolves.toMatchObject({ baseUrl: "http://127.0.0.1:4313" });
    const bad = startDevServer({
      command: "/n/node",
      args: ["x"],
      readiness: createReadiness(entries.astro.readiness, { port: 4000 }),
      appDir: root,
      port: { mode: "fixed", port: 4000 },
      readyTimeoutMs: 2000,
      logPath: join(root, "b.log"),
      spawnImpl: fakeSpawn([
        '{"message":" astro  v7.4.12 ready in 9 ms\\n┃ Local    http://127.0.0.1:4001/"}',
      ]),
    });
    await expect(bad.ready).rejects.toMatchObject({ code: "READY_CONTRADICTED" });
    await ok.closeLog();
    await bad.closeLog();
  });
});

describe("launcher-tree ownership and stop", () => {
  const L = "Sat Nov 21 03:10:31 2026";
  const ARGS = {
    100: "node /usr/lib/node_modules/npm/bin/npm-cli.js run dev -- --hostname 127.0.0.1 --port 4313",
    101: "sh -c node scripts/dev-content.mjs --hostname 127.0.0.1 --port 4313",
    102: "node /w/apps/next/scripts/dev-content.mjs --hostname 127.0.0.1 --port 4313",
    103: "node /w/apps/next/node_modules/next/dist/bin/next dev --hostname 127.0.0.1 --port 4313",
    104: "next-server (v16.4.6)",
  };
  const rows = {
    100: `100 1 100 ${L} node`,
    101: `101 100 100 ${L} sh`,
    102: `102 101 100 ${L} node`,
    103: `103 102 100 ${L} node`,
    // A grandchild in its own process group: owned through its parent chain, not the group.
    104: `104 103 104 ${L} node`,
  };

  function tree() {
    const alive = new Set([100, 101, 102, 103, 104]);
    let resolveExit;
    const handle = {
      native: { pid: 100, pgid: 100, lstart: L, comm: "node", args: ARGS[100] },
      exit: null,
      known: new Map(),
    };
    handle.exited = new Promise((r) => {
      resolveExit = r;
    });
    const psImpl = (_cmd, args) => {
      if (args[0] === "-o" && args[1] === "args=") return ARGS[Number(args[3])] ?? "";
      return [...alive].map((p) => rows[p]).join("\n");
    };
    const birthImpl = (pid) =>
      alive.has(pid)
        ? { pid, ppid: 1, pgid: pid === 104 ? 104 : 100, lstart: L, comm: "node" }
        : null;
    return { alive, handle, psImpl, birthImpl, die: (e) => resolveExit((handle.exit = e)) };
  }

  it("classifies the npm → sh → sidecar → next → next-server tree from ps args", () => {
    const t = tree();
    const owned = snapshotOwnedWithArgs(t.handle, t.psImpl, "darwin");
    expect(owned.map((p) => p.pid)).toEqual([100, 101, 102, 103, 104]);
    expect(classifyRoles(distributionOf(FIXTURE, "next"), owned).map((r) => r.role)).toEqual([
      "npm",
      "shell",
      "content-sidecar",
      "next-cli",
      "next-server",
    ]);
  });

  it("stops the whole tree: the group on SIGINT, then the out-of-group grandchild individually, and verifies absence", async () => {
    const t = tree();
    const signals = [];
    const killImpl = (target, signal) => {
      signals.push([target, signal]);
      if (target === -100 && signal === "SIGINT") {
        for (const p of [100, 101, 102, 103]) t.alive.delete(p);
        t.die({ exitCode: null, signalCode: "SIGINT" });
      }
      if (target === 104 && signal === "SIGTERM") t.alive.delete(104);
    };
    const report = await stopOwned(t.handle, {
      platform: "darwin", // These fixtures model BSD ps birth fields.
      intWaitMs: 150,
      termWaitMs: 150,
      killWaitMs: 150,
      killImpl,
      psImpl: t.psImpl,
      birthImpl: t.birthImpl,
    });
    expect(signals).toEqual([
      [-100, "SIGINT"],
      [104, "SIGTERM"],
    ]);
    expect(report.absent).toBe(true);
    expect(report.endings.find((e) => e.pid === 104)).toMatchObject({ endedAfter: "SIGTERM" });
    expect(report.endings.find((e) => e.pid === 103)).toMatchObject({ endedAfter: "SIGINT" });
  });

  it("a descendant that survives every signal leaves the stop not absent", async () => {
    const t = tree();
    const killImpl = (target, signal) => {
      if (target === -100 && signal === "SIGINT") {
        for (const p of [100, 101, 102, 103]) t.alive.delete(p);
        t.die({ exitCode: 0, signalCode: null });
      }
    };
    const report = await stopOwned(t.handle, {
      platform: "darwin", // These fixtures model BSD ps birth fields.
      intWaitMs: 100,
      termWaitMs: 100,
      killWaitMs: 100,
      killImpl,
      psImpl: t.psImpl,
      birthImpl: t.birthImpl,
    });
    expect(report.absent).toBe(false);
    expect(report.remaining.map((p) => p.pid)).toEqual([104]);
  });
});

function fixedSession(dir, { framework, adapterSha, nodeSha = "n", corpus = "d", appTree = "t" }) {
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    join(dir, "records.jsonl"),
    [
      { type: "run-start", schedule: [] },
      { type: "server-stop", report: { absent: true } },
      { type: "run-end", exitCode: 0 },
    ]
      .map((r) => JSON.stringify(r))
      .join("\n") + "\n",
  );
  writeFileSync(
    join(dir, "summary.json"),
    JSON.stringify({
      exitCode: 0,
      ownedServerAbsentAfterStop: true,
      accounting: { stopReason: null, notAttempted: 0 },
    }),
  );
  writeFileSync(
    join(dir, "receipt.json"),
    JSON.stringify({
      mode: "fixed-distribution",
      framework,
      identity: { ok: true },
      schedule: [],
      fixture: {
        kind: "field-notes",
        scale: 100,
        adapter: framework === "zfb" ? "original-memoized" : "archive",
        adapterSha256: adapterSha,
        sourcePins: { dir: "/s", datasetSha256: corpus },
        configSha256: `cfg-${framework}`,
        appTreeSha256: appTree,
        nodeModules: { sha256: `nm-${framework}` },
      },
      distribution: { node: { version: "v24", sha256: nodeSha }, install: { executables: [] } },
      server: { launch: { kind: "direct" } },
      browser: { version: "1" },
      harnessCapability: { sha: "h" },
      host: { hwModel: "Mac" },
      profile: {
        mode: "fixed-distribution",
        framework,
        distribution: { installDir: `/i/${framework}`, launch: "direct" },
        fixture: { adapter: framework === "zfb" ? "original-memoized" : "archive", scale: 100 },
        pins: { harnessTreeSha256: "h", observerSha256: "o", adapterSha256: adapterSha },
      },
    }),
  );
}

describe("aggregate: a declared framework difference (fixed cohorts only)", () => {
  const cohort = (root, over = {}) =>
    [
      ["s1-zfb", "zfb"],
      ["s2-astro", "astro"],
      ["s3-astro", "astro"],
      ["s4-zfb", "zfb"],
    ].map(([label, fw]) => {
      fixedSession(join(root, label), {
        framework: fw,
        adapterSha: `a-${fw}`,
        ...over[fw],
      });
      return { label, variant: fw, dir: join(root, label) };
    });

  it("passes with framework declared and fails without it", () => {
    const root = tmp();
    const sessions = cohort(root);
    const ok = aggregate({ sessions, declaredDifferences: ["framework"] });
    expect(ok.report.identity.checks.filter((c) => !c.ok)).toEqual([]);
    expect(ok.exitCode).toBe(0);
    const none = aggregate({ sessions, declaredDifferences: [] });
    expect(none.exitCode).toBe(3);
    expect(none.report.identity.checks.filter((c) => !c.ok).map((c) => c.name)).toEqual(
      expect.arrayContaining([
        "across-variants:framework",
        "across-variants:adapter",
        "across-variants:profile",
      ]),
    );
  });

  it("a framework declaration never excuses a corpus or node difference", () => {
    const root = tmp();
    const sessions = cohort(root, { astro: { corpus: "other", nodeSha: "other-node" } });
    const r = aggregate({ sessions, declaredDifferences: ["framework"] });
    expect(r.exitCode).toBe(3);
    expect(r.report.identity.checks.filter((c) => !c.ok).map((c) => c.name)).toEqual([
      "across-variants:corpus",
      "across-variants:toolchain",
    ]);
  });

  it("framework cannot be declared for source-built sessions", () => {
    const root = tmp();
    const sessions = cohort(root);
    const rec = readJson(join(sessions[0].dir, "receipt.json"));
    writeFileSync(
      join(sessions[0].dir, "receipt.json"),
      JSON.stringify({ ...rec, mode: "source-built" }),
    );
    const r = aggregate({ sessions, declaredDifferences: ["framework"] });
    expect(
      r.report.identity.checks.find(
        (c) => c.name === "framework-difference-only-for-fixed-distribution",
      ),
    ).toMatchObject({ ok: false, actual: ["s1-zfb"] });
  });
});

describe("series: fixed pairwise cohorts", () => {
  function plan(root, declared, b = "astro", installs = shared().installs) {
    const write = (fw, n) => {
      const p = example(
        fw === "zfb"
          ? "field-notes-fixed-zfb-original-memoized-100.example.json"
          : `field-notes-fixed-${fw}-100.example.json`,
      );
      p.outDir = join(root, "out");
      p.distribution.installDir = installs[fw].out;
      writeFileSync(join(root, n), JSON.stringify(p));
    };
    write("zfb", "z.json");
    write(b, "b.json");
    const plan = {
      schema: "dev-browser-observe.series.v1",
      id: "c",
      protocol: "dev-browser-observe.timing-abba.v1",
      variants: { zfb: { profile: "z.json" }, [b]: { profile: "b.json" } },
      declaredDifferences: declared,
      order: ["zfb", b, b, "zfb"],
      labelPrefix: `c-${b}`,
    };
    writeFileSync(join(root, "plan.json"), JSON.stringify(plan));
    return loadSeriesPlan(join(root, "plan.json"));
  }

  it("zfb vs astro and zfb vs next load with framework declared, and need it", () => {
    expect(plan(tmp(), ["framework"]).errors).toEqual([]);
    expect(plan(tmp(), ["framework"], "next").errors).toEqual([]);
    expect(plan(tmp(), []).errors.join("\n")).toMatch(/different frameworks; declare framework/);
    for (const n of [
      "series-fixed-zfb-vs-astro.example.json",
      "series-fixed-zfb-vs-next.example.json",
    ])
      expect(validateAs("series", example(n)), n).toEqual([]);
  });

  it("framework is refused for source-built variants", () => {
    const root = tmp();
    const p = example("field-notes-direct-read-timing-abba-5000.example.json");
    for (const n of ["a.json", "b.json"])
      writeFileSync(join(root, n), JSON.stringify({ ...p, outDir: join(root, "o") }));
    writeFileSync(
      join(root, "plan.json"),
      JSON.stringify({
        schema: "dev-browser-observe.series.v1",
        id: "x",
        protocol: "dev-browser-observe.timing-abba.v1",
        variants: { A: { profile: "a.json" }, B: { profile: "b.json" } },
        declaredDifferences: ["framework"],
        order: ["A", "B", "B", "A"],
        labelPrefix: "x",
      }),
    );
    expect(loadSeriesPlan(join(root, "plan.json")).errors.join("\n")).toMatch(
      /framework applies only to fixed-distribution/,
    );
  });
});

describe("node identity", () => {
  it("records the realpath, sha256 and version of this node", () => {
    const n = nodeIdentity();
    expect(n.version).toBe(process.version);
    expect(n.sha256).toMatch(/^[0-9a-f]{64}$/);
  });
});

describe("review fixes (fixed distributions)", () => {
  it("the fixed verdict carries the frozen-copy checks: a broken playwright link in a frozen copy is refused", () => {
    const { s, installs } = shared();
    const { path } = fixedProfile(s, "astro", installs.astro.out, {}, "frozen-astro.json");
    const { profile, resolved } = loadProfile(path);
    const manifest = loadFieldNotesFixture(resolved.fixtureDir);
    const root = tmp();
    const tiny = makeTinyHarness(root);
    const r = freezeHarness({
      outDir: join(root, "frozen"),
      harnessDir: tiny.harness,
      allowDirty: true,
      capabilityImpl: () => ({ repo: "/r", sha: "f".repeat(40), harnessDirDirty: false }),
      playwrightDir: tiny.playwright,
    });
    try {
      const ok = fixedIdentityVerdict({ profile, resolved, manifest, harnessDir: r.dir });
      expect(
        ok.checks.filter((c) => c.name.startsWith("harness-frozen")).map((c) => [c.name, c.ok]),
      ).toEqual([
        ["harness-frozen-copy-intact", true],
        ["harness-frozen-playwright-link", true],
      ]);
      rmSync(r.stamp.playwright.link);
      symlinkSync(tmpdir(), r.stamp.playwright.link, "dir");
      const bad = fixedIdentityVerdict({ profile, resolved, manifest, harnessDir: r.dir });
      expect(bad.ok).toBe(false);
      expect(failed(bad)).toEqual(["harness-frozen-playwright-link"]);
      expect(
        fixedIdentityVerdict({ profile, resolved, manifest }).checks.some((c) =>
          c.name.startsWith("harness-frozen"),
        ),
      ).toBe(false);
    } finally {
      unfreezeForRemoval(r.dir);
    }
  });

  it("npm-script launch requires the npm that installed the app", () => {
    const s = setup();
    const npmDir = join(s.root, "npm");
    writeTree(npmDir, {
      "bin/npm-cli.js": "// npm\n",
      "package.json": JSON.stringify({ version: "11.0.0" }),
    });
    const npmCli = join(npmDir, "bin", "npm-cli.js");
    const out = join(s.root, "install-npm");
    installDistribution({
      fixtureDir: s.fixtureDir,
      framework: "next",
      sourceDir: s.src,
      outDir: out,
      npmCli,
      spawnImpl: (_c, _a, opts) => {
        fakeNpmCi(opts.cwd, "next");
        return { status: 0, stdout: "", stderr: "" };
      },
    });
    const { path, profile } = fixedProfile(s, "next", out);
    writeFileSync(
      path,
      JSON.stringify({ ...profile, distribution: { installDir: out, launch: "npm-script" } }),
    );
    const v = (impl) => {
      const loaded = loadProfile(path);
      return fixedIdentityVerdict({
        profile: loaded.profile,
        resolved: loaded.resolved,
        manifest: loadFieldNotesFixture(loaded.resolved.fixtureDir),
        npmCliImpl: impl,
      });
    };
    expect(failed(v(() => npmCli))).toEqual([]);
    writeFileSync(npmCli, "// a different npm\n");
    expect(failed(v(() => npmCli))).toEqual(["npm-matches-install"]);
    expect(failed(v(() => null))).toEqual(["npm-cli", "npm-matches-install"]);
  });

  it("processExecutable reads lsof's txt entry, falls back to an absolute ps comm, else null", () => {
    const lsof = (cmd) => {
      if (cmd === "lsof") return "p42\nftxt\nn/opt/node/bin/node\nftxt\nn/usr/lib/dyld\n";
      throw new Error("no ps");
    };
    expect(processExecutable(42, lsof)).toEqual({ path: "/opt/node/bin/node", method: "lsof-txt" });
    const ps = (cmd) => {
      if (cmd === "lsof") throw new Error("no lsof");
      return "/usr/local/bin/node\n";
    };
    expect(processExecutable(42, ps)).toEqual({ path: "/usr/local/bin/node", method: "ps-comm" });
    expect(processExecutable(42, (cmd) => (cmd === "ps" ? "node\n" : ""))).toEqual({
      path: null,
      method: "unavailable",
    });
  });

  it("process identity: node roles need the install's node, zfb-native its binary, others the app's node_modules", () => {
    const root = tmp();
    const app = join(root, "app");
    writeTree(app, {
      "node_modules/@takazudo/zfb-darwin-arm64/zfb": "native",
      "node_modules/@esbuild/darwin-arm64/bin/esbuild": "esbuild",
    });
    writeTree(root, { "other/node": "other node", "other/zfb": "other native" });
    const nativeSha = sha256File(join(app, "node_modules/@takazudo/zfb-darwin-arm64/zfb"));
    const exes = {
      1: process.execPath,
      2: join(root, "other/node"),
      3: join(app, "node_modules/@takazudo/zfb-darwin-arm64/zfb"),
      4: join(root, "other/zfb"),
      5: join(app, "node_modules/@esbuild/darwin-arm64/bin/esbuild"),
      6: "/bin/sh",
      7: join(root, "other/tool"),
    };
    const roles = [
      { pid: 1, role: "zfb-shim" },
      { pid: 2, role: "npm" },
      { pid: 3, role: "zfb-native" },
      { pid: 4, role: "zfb-native" },
      { pid: 5, role: "unclassified" },
      { pid: 6, role: "shell" },
      { pid: 7, role: "unclassified" },
      { pid: 8, role: "next-server" },
      { pid: 9, role: "next-cli" },
    ];
    const checks = processIdentityChecks({
      processes: roles.map((r) => ({ pid: r.pid })),
      roles,
      nodeRealpath: realpathSync(process.execPath),
      executables: [{ role: "zfb-native", sha256: nativeSha }],
      appDir: app,
      exeOf: (pid) =>
        exes[pid] ? { path: exes[pid], method: "fake" } : { path: null, method: "unavailable" },
      alive: (pid) => pid === 9,
    });
    expect(checks.map((c) => [c.pid, c.ok])).toEqual([
      [1, true],
      [2, false],
      [3, true],
      [4, false],
      [5, true],
      [6, true],
      [7, false],
      [8, true],
      [9, false],
    ]);
    expect(checks.find((c) => c.pid === 8).note).toMatch(/exited/);
  });

  it("a fixed run whose server tree runs PATH's node is refused at readiness, before the browser and any edit", async () => {
    const { s, installs } = shared();
    const { path } = fixedProfile(s, "next", installs.next.out, {}, "path-node.json");
    let resolveExit;
    const handle = {
      native: { pid: 100, pgid: 100, lstart: "x", comm: "node" },
      exit: null,
      known: new Map(),
    };
    handle.exited = new Promise((r) => {
      resolveExit = r;
    });
    let browserLaunched = false;
    const code = await execute({
      profilePath: path,
      label: "PN1",
      command: "run",
      stderr: () => {},
      hooks: {
        freePort: async () => 4556,
        startDevServer: (opts) => ({
          handle,
          args: opts.args,
          envOverrides: opts.env,
          zfbEnv: {},
          removedDiagnosticEnv: [],
          diagnostic: false,
          startedNs: process.hrtime.bigint(),
          ready: Promise.resolve({
            baseUrl: "http://127.0.0.1:4556",
            readyNs: process.hrtime.bigint(),
          }),
          closeLog: async () => ({}),
        }),
        snapshotOwned: () => [
          { pid: 100, ppid: 1, comm: "node", args: "node /w/apps/next/scripts/dev-content.mjs" },
          {
            pid: 101,
            ppid: 100,
            comm: "node",
            args: "node /w/node_modules/next/dist/bin/next dev",
          },
        ],
        processExecutable: (pid) => ({
          path: pid === 101 ? "/opt/other/bin/node" : process.execPath,
          method: "fake",
        }),
        processAlive: () => true,
        launchOwnedBrowser: async () => {
          browserLaunched = true;
          throw new Error("must not launch");
        },
        stopOwned: async () => {
          resolveExit((handle.exit = { exitCode: 0, signalCode: null }));
          return { absent: true };
        },
      },
    });
    expect(code).toBe(EXIT.identityRejected);
    expect(browserLaunched).toBe(false);
    const dir = join(s.root, "out", "PN1");
    const records = readFileSync(join(dir, "records.jsonl"), "utf8")
      .trim()
      .split("\n")
      .map((l) => JSON.parse(l));
    expect(records.some((r) => r.type === "attempt")).toBe(false);
    expect(
      records
        .find((r) => r.type === "identity" && r.phase === "server-ready")
        .checks.filter((c) => !c.ok),
    ).toMatchObject([{ pid: 101, role: "next-cli", actual: "/opt/other/bin/node" }]);
    expect(readJson(join(dir, "summary.json")).identityRejected).toBe(true);
  });

  it("a `sh -c` must run a system shell; a byte-identical node elsewhere is the same node", () => {
    const root = tmp();
    writeTree(root, {
      "fake/sh": "not a shell",
      "node-a": "node bytes",
      "node-b": "node bytes",
      "node-c": "other",
    });
    const roles = [
      { pid: 1, role: "shell" },
      { pid: 2, role: "shell" },
      { pid: 3, role: "next-cli" },
      { pid: 4, role: "next-cli" },
    ];
    const exes = {
      1: "/bin/sh",
      2: join(root, "fake/sh"),
      3: join(root, "node-b"),
      4: join(root, "node-c"),
    };
    const checks = processIdentityChecks({
      processes: roles.map((r) => ({ pid: r.pid })),
      roles,
      nodeRealpath: join(root, "node-a"),
      nodeSha256: sha256File(join(root, "node-a")),
      appDir: root,
      exeOf: (pid) => ({ path: exes[pid], method: "fake" }),
      alive: () => true,
    });
    expect(checks.map((c) => [c.pid, c.ok])).toEqual([
      [1, true],
      [2, false],
      [3, true],
      [4, false],
    ]);
    expect(checks[2].matchedBy).toBe("sha256");
    expect(
      processIdentityChecks({
        processes: [{ pid: 1 }],
        roles: [{ pid: 1, role: "shell" }],
        nodeRealpath: "/n",
        appDir: root,
        systemShells: ["/no/such/shell"],
        exeOf: () => ({ path: "/bin/sh", method: "fake" }),
      })[0].ok,
    ).toBe(false);
  });

  it("a worker born after readiness that runs PATH's node stops the batch before the first edit", async () => {
    const { s, installs } = shared();
    const { path, profile } = fixedProfile(s, "next", installs.next.out, {}, "late-node.json");
    profile.scenarios = [
      {
        id: "body",
        set: "workload",
        warmOrder: "none",
        ops: [{ id: "b", op: "edit-body", article: "article-00005", writeMethod: "in-place" }],
      },
    ];
    writeFileSync(path, JSON.stringify(profile));
    let resolveExit;
    const handle = {
      native: { pid: 100, pgid: 100, lstart: "L", comm: "node" },
      exit: null,
      known: new Map(),
    };
    handle.exited = new Promise((r) => {
      resolveExit = r;
    });
    const tree = [
      {
        pid: 100,
        ppid: 1,
        lstart: "L",
        comm: "node",
        args: "node /w/apps/next/scripts/dev-content.mjs",
      },
      {
        pid: 101,
        ppid: 100,
        lstart: "L",
        comm: "node",
        args: "node /w/node_modules/next/dist/bin/next dev",
      },
    ];
    let snapshots = 0;
    const page = fakePage({
      onWait: () =>
        Promise.resolve({
          kind: "notify",
          payload: { docUUID: "doc-1" },
          t1: process.hrtime.bigint() + 1_000_000n,
        }),
    });
    const contentBefore = readFileSync(join(s.src, "content/articles/article-00005.md"), "utf8");
    const code = await execute({
      profilePath: path,
      label: "LATE1",
      command: "run",
      stderr: () => {},
      hooks: {
        freePort: async () => 4557,
        startDevServer: (opts) => ({
          handle,
          args: opts.args,
          envOverrides: opts.env,
          zfbEnv: {},
          removedDiagnosticEnv: [],
          diagnostic: false,
          startedNs: process.hrtime.bigint(),
          ready: Promise.resolve({
            baseUrl: "http://127.0.0.1:4557",
            readyNs: process.hrtime.bigint(),
          }),
          closeLog: async () => ({}),
          isAlive: () => ({ alive: true, exit: null }),
          pid: () => 100,
          waitExit: () => handle.exited,
          sampleResources: () => ({ summedRssKiB: 1 }),
          logMark: () => 0,
          logSince: () => ({ lineCount: 0, lines: [] }),
        }),
        snapshotOwned: () => {
          snapshots += 1;
          // A next-server worker appears after the readiness snapshot.
          return snapshots === 1
            ? tree
            : [
                ...tree,
                { pid: 102, ppid: 101, lstart: "L2", comm: "node", args: "next-server (v16.4.6)" },
              ];
        },
        processExecutable: (pid) => ({
          path: pid === 102 ? "/opt/path/bin/node" : process.execPath,
          method: "fake",
        }),
        processAlive: () => true,
        launchOwnedBrowser: async () => ({ observer: page, identity: { engine: "fake" } }),
        closeOwnedBrowser: async () => [],
        stopOwned: async () => {
          resolveExit((handle.exit = { exitCode: 0, signalCode: null }));
          return { absent: true };
        },
      },
    });
    expect(code).toBe(EXIT.identityRejected);
    const dir = join(s.root, "out", "LATE1");
    const records = readFileSync(join(dir, "records.jsonl"), "utf8")
      .trim()
      .split("\n")
      .map((l) => JSON.parse(l));
    expect(records.some((r) => r.type === "attempt")).toBe(false);
    expect(records.find((r) => r.type === "precheck")).toMatchObject({
      outcome: "identity-rejected",
      halt: {
        reason: "process-identity",
        checks: [{ pid: 102, role: "next-server", actual: "/opt/path/bin/node" }],
      },
    });
    expect(records.find((r) => r.type === "identity" && r.phase === "before-edit").ok).toBe(false);
    const summary = readJson(join(dir, "summary.json"));
    expect(summary).toMatchObject({ identityRejected: true });
    expect(summary.accounting).toMatchObject({
      attempted: 0,
      notAttempted: 1,
      stopReason: "identity-rejected",
    });
    expect(readFileSync(join(dir, "work/content/articles/article-00005.md"), "utf8")).toBe(
      contentBefore,
    );
  });

  it("an unparsable ready URL fails readiness instead of throwing", () => {
    const r = createReadiness({ url: "ready on (\\S+)", require: [] }, { port: 1 });
    expect(r.push("ready on http://[not-a-host")).toEqual({
      failed: { reason: "ready-url-unparsable", printed: "http://[not-a-host" },
    });
  });

  it("framework must actually differ when it is declared (aggregate and series)", () => {
    const root = tmp();
    const sessions = ["s1", "s2"].map((label, i) => {
      fixedSession(join(root, label), { framework: "zfb", adapterSha: "a" });
      return { label, variant: i === 0 ? "A" : "B", dir: join(root, label) };
    });
    const r = aggregate({ sessions, declaredDifferences: ["framework"] });
    expect(
      r.report.identity.checks.find((c) => c.name === "framework-declared-and-differs"),
    ).toMatchObject({
      ok: false,
    });
    expect(r.exitCode).toBe(3);
    const { installs } = shared();
    const proot = tmp();
    const p = example("field-notes-fixed-zfb-original-memoized-100.example.json");
    p.outDir = join(proot, "out");
    p.distribution.installDir = installs.zfb.out;
    for (const n of ["a.json", "b.json"]) writeFileSync(join(proot, n), JSON.stringify(p));
    writeFileSync(
      join(proot, "plan.json"),
      JSON.stringify({
        schema: "dev-browser-observe.series.v1",
        id: "x",
        protocol: "dev-browser-observe.timing-abba.v1",
        variants: { A: { profile: "a.json" }, B: { profile: "b.json" } },
        declaredDifferences: ["framework"],
        order: ["A", "B", "B", "A"],
        labelPrefix: "x",
      }),
    );
    expect(loadSeriesPlan(join(proot, "plan.json")).errors.join("\n")).toMatch(
      /framework is declared but every variant runs the same framework/,
    );
  });

  it("series compares the install receipts at plan time: an undeclared node or build difference fails before any session", () => {
    const s = setup();
    const a = install(s, "zfb");
    const bOut = join(s.root, "install-zfb-b");
    installDistribution({
      fixtureDir: s.fixtureDir,
      framework: "zfb",
      sourceDir: s.src,
      outDir: bOut,
      npmCli: "/fake/npm-cli.js",
      spawnImpl: (_c, _a, opts) => {
        fakeNpmCi(opts.cwd, "zfb");
        writeFileSync(join(opts.cwd, "node_modules", "@takazudo", "zfb", "extra.txt"), "x");
        return { status: 0, stdout: "", stderr: "" };
      },
    });
    const proot = tmp();
    const p = example("field-notes-fixed-zfb-original-memoized-100.example.json");
    p.outDir = join(proot, "out");
    const write = (n, dir) =>
      writeFileSync(
        join(proot, n),
        JSON.stringify({ ...p, distribution: { installDir: dir, launch: "direct" } }),
      );
    write("a.json", a.out);
    write("b.json", bOut);
    const plan = (declared) => {
      writeFileSync(
        join(proot, "plan.json"),
        JSON.stringify({
          schema: "dev-browser-observe.series.v1",
          id: "aa",
          protocol: "dev-browser-observe.timing-abba.v1",
          variants: { A: { profile: "a.json" }, B: { profile: "b.json" } },
          declaredDifferences: declared,
          order: ["A", "B", "B", "A"],
          labelPrefix: "aa",
        }),
      );
      return loadSeriesPlan(join(proot, "plan.json")).errors.join("\n");
    };
    expect(plan([])).toMatch(/install receipts differ in build/);
    expect(plan(["build"])).toBe("");
    const rb = readJson(join(bOut, "install-receipt.json"));
    rb.node.sha256 = "0".repeat(64);
    writeFileSync(join(bOut, "install-receipt.json"), JSON.stringify(rb));
    expect(plan(["build"])).toMatch(/install receipts differ in toolchain/);
    rmSync(join(bOut, "install-receipt.json"));
    expect(plan(["build", "toolchain"])).toMatch(/install receipt unreadable/);
  });

  it("receipt.v1: productSha is a required SHA string for source-built and null for fixed-distribution", () => {
    const base = {
      schema: "dev-browser-observe.receipt.v1",
      label: "l",
      command: "run",
      createdAt: "t",
      harnessCapability: { sha: "s", harnessDirDirty: false },
      identity: { ok: true, checks: [] },
      host: {},
    };
    expect(validateAs("receipt", { ...base, productSha: "a".repeat(40) })).toEqual([]);
    expect(
      validateAs("receipt", { ...base, mode: "source-built", productSha: null }).join("\n"),
    ).toMatch(/productSha: expected string/);
    expect(validateAs("receipt", { ...base, productSha: null }).join("\n")).toMatch(
      /productSha: expected string/,
    );
    expect(
      validateAs("receipt", {
        ...base,
        mode: "fixed-distribution",
        framework: "astro",
        distribution: {},
        productSha: null,
      }),
    ).toEqual([]);
    expect(
      validateAs("receipt", {
        ...base,
        mode: "fixed-distribution",
        framework: "astro",
        distribution: {},
        productSha: "a".repeat(40),
      }).join("\n"),
    ).toMatch(/productSha: expected null/);
  });
});

it.each([
  "footer-v1-refuse",
  "footer-v2-joined",
  "footer-v2-idle-deferred",
  "v1-accept",
  "v1-refuse",
  "joined",
  "idle-deferred",
  "no-release",
  "changed",
  "request",
  "navigated",
  "transient",
  "abort-join",
  "failed",
  "expired",
  "hung",
])(
  "normal ZFB family controller reaches blank setup before full custody (release %s)",
  async (mode) => {
    const cancellation = new AbortController();
    const footer = mode.startsWith("footer-");
    const v2 = !mode.includes("v1");
    const released = ["v1-accept", "joined", "idle-deferred"].includes(mode);
    // An idle capture never admits on its accepted sample here: it defers and joins the next one.
    const deferred = mode.endsWith("-deferred");
    const { s, installs } = shared();
    const label = "family-" + mode;
    const { path, profile } = fixedProfile(s, "zfb", installs.zfb.out, {}, label + ".json");
    profile.fixedRssSafety = "linux-fixed-zfb.v4";
    profile.fixedFamilyAdmission = v2 ? "blank-join-owned.v2" : "blank-before-custody.v1";
    profile.scenarios = [
      {
        id: "fn-rapid-add-delete",
        set: "workload",
        warmOrder: "none",
        ops: [
          {
            id: "rapid-add-delete",
            op: "rapid-sequence",
            variant: "add-then-delete",
            writeMethod: "create",
            spacingMs: 100,
          },
        ],
      },
    ];
    if (footer) {
      delete profile.fixedFamilyAdmission;
      profile.fixedZfbFooter = v2
        ? "original-shared-layout-join-owned.v2"
        : "original-shared-layout.v1";
      profile.fixedPeerBrowser = {
        executablePath: "/controlled/chrome",
        executableSha256: "a".repeat(64),
        defaultExecutableSha256: "b".repeat(64),
      };
      profile.scenarios = [
        {
          id: "fn-shared-component",
          set: "workload",
          warmOrder: "none",
          ops: [
            { id: "footer", op: "edit-shared-component", writeMethod: "in-place", targetPath: "/" },
          ],
        },
      ];
    }
    profile.server.readyTimeoutMs = 300000;
    profile.limits.wallCeilingMs = 7200000;
    Object.assign(profile.timeouts, { attemptMs: 60000, supplementalMs: 60000, restoreMs: 120000 });
    writeFileSync(path, JSON.stringify(profile));
    const birth = (pid, ppid, pgid) => ({
      pid,
      ppid,
      pgid,
      state: "S",
      startTicks: String(pid),
      bootId: "boot",
      birthVersion: "linux-proc-v1",
      comm: "node",
    });
    const root = birth(100, 99, 100),
      dev = birth(101, 100, 100),
      worker = birth(102, 101, 102);
    const known = new Map([root, dev, worker].map((b) => [b.pid, b])),
      events = [];
    const proof = {
      root,
      dev,
      worker,
      sample: {
        live: [root, dev],
        cadenceDeadlineNs: String(process.hrtime.bigint() + 1000000000n),
      },
    };
    let context,
      pages = [];
    const native = join(installs.zfb.out, "apps/zfb/node_modules/@takazudo/zfb-linux-x64-gnu/zfb");
    const live = [
      { ...root, args: "node /node_modules/@takazudo/zfb/bin/zfb.mjs dev" },
      { ...dev, args: native + " dev" },
    ];
    const page = fakePage(),
      handle = { native: root, launcher: "none", exit: null, known };
    if (footer) {
      page.url = (p) => "http://127.0.0.1:4555" + p;
      page.prepare = async (p) => {
        page.prepared.push(p);
        return { httpStatus: 200, docUUID: "home", url: page.url(p) };
      };
      page.arm = async () => ({
        armed: true,
        docUUID: "home",
        observedUrl: page.url("/"),
        actual: [1, "Same content. Same design. Three build systems."],
      });
      page.peerSocketTracking = new Map([
        ["home", { requestId: "home", status: 101, url: "ws://127.0.0.1:4555/__zfb/reload/ws" }],
      ]);
    }
    const start = Number(process.hrtime.bigint() / 1000000n);
    vi.spyOn(lockModule, "acquireLock").mockResolvedValue({
      release: async () => events.push("lock-closed"),
    });
    vi.spyOn(custodyModule, "startFixedCustody").mockReturnValue({
      ready: Promise.resolve(),
      attachStartup() {},
      stop: async () => events.push("holder-closed"),
    });
    vi.spyOn(familyModule, "verifyFamilyImages").mockImplementation(() => {});
    vi.spyOn(safetyModule, "startFixedRssSafety").mockReturnValue({
      async verifyFamilyBrowserReady() {
        events.push("rss-ready");
      },
      captureFamilyCycle(receipt, options = {}) {
        events.push("check");
        if (mode === "no-release") throw Error("release unavailable");
        if (footer) {
          events.push("blank-capture");
          expect(page.prepared).toEqual([]);
        }
        if (deferred && !receipt.idleDeferral) {
          expect(options).toEqual({ idleNeedsFullPeriod: true });
          events.push("idle-deferred");
          return {
            before: { ...proof, sample: { ...proof.sample, epoch: 3 } },
            settlement: null,
            deferred: { reserveNs: 1_000_000_000n, remainingCadenceNs: 100_000_000n },
          };
        }
        const now = process.hrtime.bigint();
        const start = String(now);
        const deadline = String(
          now + (mode === "expired" ? -1n : mode === "hung" ? 25_000_000n : 750_000_000n),
        );
        receipt.cycle = {
          epoch: 4,
          startedNs: start,
          helper: {
            pid: 103,
            spawnHrtimeNs: start,
            entryNs: start,
            deadlineNs: deadline,
          },
        };
        const before = { ...proof, sample: { ...proof.sample, epoch: 3 } };
        const settlement = new Promise((resolve) =>
          setImmediate(() => {
            if (mode === "abort-join") cancellation.abort(Error("cancel pending admission"));
            if (mode === "transient") pages[0].emit("framenavigated");
            if (mode === "request") context.emit("request", { url: () => "http://localhost/" });
            if (mode === "navigated") pages = [{ url: () => "http://localhost/" }];
            if (mode === "hung") return;
            Object.assign(proof.sample, {
              epoch: mode === "changed" ? 5 : 4,
              startedNs: start,
              helperPid: 103,
              helperSpawnNs: start,
              entryNs: start,
              deadlineNs: deadline,
              resultNs: String(process.hrtime.bigint()),
            });
            resolve();
          }),
        );
        return { before, settlement };
      },
      nextFamilyHelper() {
        events.push("next-helper");
        return new Promise((resolve) => setImmediate(resolve));
      },
      verifyFamilyAdmission() {
        events.push("check");
        if (footer && page.prepared.length) {
          events.push("refused-before-write");
          throw Error("controlled pending at pre-write");
        }
        if (mode === "footer-v1-refuse" || mode === "v1-refuse" || mode === "failed")
          throw Error("release unavailable");
        return proof;
      },
      async stop() {
        events.push("sampler-closed");
        return null;
      },
      report() {
        return {};
      },
    });
    const stderr = [];
    try {
      const code = await execute({
        profilePath: path,
        label,
        command: "run",
        supervisorId: "controlled",
        signal: cancellation.signal,
        supervisorStartedMs: start,
        workDeadline: start + 7140000,
        supervisorDeadlineMs: start + 7200000,
        stderr: (s) => stderr.push(s),
        hooks: {
          freePort: async () => 4555,
          startDevServer: () => ({
            handle,
            zfbEnv: {},
            envOverrides: {},
            removedDiagnosticEnv: [],
            args: [],
            startedNs: process.hrtime.bigint(),
            ready: Promise.resolve({
              baseUrl: "http://127.0.0.1:4555",
              readyNs: process.hrtime.bigint(),
            }),
            closeLog: async () => ({ pipes: "closed" }),
            isAlive: () => ({ alive: true }),
            pid: () => 100,
            waitExit: () => new Promise(() => {}),
            sampleResources: () => ({ summedRssKiB: 1 }),
            sampleResourcesAsync: async () => ({ summedRssKiB: 1 }),
          }),
          snapshotOwned: () => live,
          processExecutable: (pid) => ({
            path: pid === 100 ? process.execPath : pid === 101 ? native : null,
            method: "controlled",
          }),
          processAlive: (pid) => pid !== 102,
          launchOwnedBrowser: async (_cfg, { onBlankContext }) => {
            events.push("blank");
            const c = new EventEmitter();
            context = c;
            c.pages = () => pages;
            onBlankContext(c);
            pages = [Object.assign(new EventEmitter(), { url: () => "about:blank" })];
            return {
              observer: page,
              identity: { engine: "fake", ...(footer && { executableSha256: "b".repeat(64) }) },
            };
          },
          closeOwnedBrowser: async () => {
            events.push("browser-closed");
            return [];
          },
          stopOwned: async () => {
            events.push("server-closed");
            return { absent: true };
          },
        },
      });
      const dir = join(s.root, "out", label);
      const records = readFileSync(join(dir, "records.jsonl"), "utf8")
        .trim()
        .split("\n")
        .map(JSON.parse);
      expect(
        events.slice(0, 3),
        JSON.stringify({ stderr, events, errors: records.filter((r) => r.error) }),
      ).toEqual(["rss-ready", "blank", "check"]);
      expect(events).toEqual(
        expect.arrayContaining([
          "server-closed",
          "sampler-closed",
          "browser-closed",
          "holder-closed",
          "lock-closed",
        ]),
      );
      expect(safetyModule.startFixedRssSafety.mock.calls[0][0].joinOwnedFamily).toBe(v2);
      if (footer) {
        const { report } = aggregate({ sessions: [{ label, dir, variant: "stock-footer" }] });
        expect(report.sessions[0]).toMatchObject({ timingExcluded: true, blocks: [] });
        expect(Object.keys(report.pooled)).toHaveLength(0);
      }
      if (footer && v2) {
        expect(code).not.toBe(EXIT.ok);
        expect(events.filter((e) => e === "blank-capture")).toHaveLength(deferred ? 2 : 1);
        expect(events, JSON.stringify(records.filter((r) => r.error))).toContain(
          "refused-before-write",
        );
        const gate = records.find((r) => r.type === "scenario-start").familyAdmission;
        expect(gate.setup.settlementOutcome).toBe("accepted");
        if (deferred) expect(gate.setup.idleDeferral.outcome).toBe("joined");
        expect(gate.pageUrl).toBe("about:blank");
        expect(records.some((r) => r.type === "fixed-peer-edit-disk" || r.writePerformed)).toBe(
          false,
        );
        expect(records.some((r) => r.type === "fixed-peer-prewrite")).toBe(true);
      } else if (released) {
        expect(code, JSON.stringify({ stderr, errors: records.filter((r) => r.error) })).toBe(
          EXIT.ok,
        );
        if (deferred) {
          const gate = records.find((r) => r.type === "scenario-start").familyAdmission;
          expect(gate.setup.idleDeferral).toMatchObject({
            outcome: "joined",
            reserveNs: "1000000000",
          });
          expect(events.indexOf("idle-deferred")).toBeLessThan(events.indexOf("next-helper"));
        }
        expect(records.find((r) => r.type === "attempt")).toMatchObject({
          mode: "final-state",
          writePerformed: true,
        });
      } else {
        expect(code).not.toBe(EXIT.ok);
        expect(page.prepared).toEqual([]);
        expect(records.some((r) => r.type === "cleanup" || r.type === "attempt")).toBe(false);
        expect(records.find((r) => r.type === "original-family-admission")).toMatchObject({
          admitted: false,
          httpRequests: mode === "request" ? 1 : 0,
          workloadWriteCalls: 0,
          snapshots: 0,
        });
      }
    } finally {
      vi.restoreAllMocks();
    }
  },
);
