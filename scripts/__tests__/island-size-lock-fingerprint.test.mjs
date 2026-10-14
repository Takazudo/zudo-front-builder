// Dependency-graph lock fingerprint for the island-size contracts (#4190).
//
// L1 over research/v3-island-size/lock-fingerprint.mjs, driven by a small
// hand-authored two-document lock (fixtures/island-size-lock/) and by the
// real pnpm-lock.yaml. Invariance cases prove the fingerprint ignores lock
// content outside LOCK-FINGERPRINT-POLICY.md's inclusion set while the raw
// file hash moves; sensitivity cases prove every included input moves it;
// fail-closed cases prove a malformed lock throws a named error instead of
// returning a hash.
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { afterEach, describe, expect, it } from "vite-plus/test";

import {
  FINGERPRINT_PARSER,
  FINGERPRINT_POLICY_VERSION,
  LockFingerprintError,
  SELECTED_IMPORTERS,
  computeLockFingerprint,
  fingerprintHelperSha256,
} from "../../research/v3-island-size/lock-fingerprint.mjs";
import { computeWithHelperSource } from "../rebaseline-island-size-linux.mjs";
import { addPackage, mutate, renameVersion } from "./fixtures/island-size-lock-mutations.mjs";

const testDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(testDir, "../..");
const helperPath = join(repoRoot, "research/v3-island-size/lock-fingerprint.mjs");
const fixtureLock = readFileSync(join(testDir, "fixtures/island-size-lock/pnpm-lock.yaml"), "utf8");
const FIXTURE_VERSION = "1.0.0";
const realLock = readFileSync(join(repoRoot, "pnpm-lock.yaml"), "utf8");
const REAL_VERSION = JSON.parse(
  readFileSync(join(repoRoot, "packages/zfb/package.json"), "utf8"),
).version;
const PLATFORM_IMPORTERS = [
  "packages/zfb-darwin-arm64",
  "packages/zfb-darwin-x64",
  "packages/zfb-linux-arm64-gnu",
  "packages/zfb-linux-x64-gnu",
  "packages/zfb-win32-x64-msvc",
];
// The packages' importers resolve vite-plus against typescript@7.0.2 and the
// excluded root importer against typescript@6.0.3, so each peer variant has
// snapshots only one side reaches.
const PACKAGE_MOCKER =
  "@vitest/mocker@5.0.1(@types/node@22.19.17)(esbuild@0.28.1)(tsx@4.23.1)(typescript@7.0.2)(yaml@2.9.0)";
const ROOT_VITE_PLUS =
  "vite-plus@1.0.0(@types/node@22.19.17)(@vitest/ui@5.0.1)(esbuild@0.28.1)(happy-dom@15.11.7)(tsx@4.23.1)(typescript@6.0.3)(yaml@2.9.0)";
const tempRoots = [];

afterEach(() => {
  while (tempRoots.length > 0) rmSync(tempRoots.pop(), { recursive: true, force: true });
});

function sha(text) {
  return createHash("sha256").update(text).digest("hex");
}

function tempDir() {
  const directory = mkdtempSync(join(tmpdir(), "zfb-lock-fingerprint-"));
  tempRoots.push(directory);
  return directory;
}

function fixtureHash(lockText, options = {}) {
  return computeLockFingerprint({ lockText, currentVersion: FIXTURE_VERSION, ...options }).sha256;
}

function realHash(lockText, options = {}) {
  return computeLockFingerprint({ lockText, currentVersion: REAL_VERSION, ...options }).sha256;
}

function expectFailure(run, code) {
  let caught;
  try {
    run();
  } catch (error) {
    caught = error;
  }
  expect(caught, `expected ${code}`).toBeInstanceOf(LockFingerprintError);
  expect(caught.code).toBe(code);
}

function expectInvariant(baselineText, mutatedText, hash, options = {}) {
  expect(sha(mutatedText)).not.toBe(sha(baselineText));
  expect(hash(mutatedText, options)).toBe(hash(baselineText));
}

function expectSensitive(baselineText, mutatedText, hash, options = {}) {
  expect(hash(mutatedText, options)).not.toBe(hash(baselineText));
}

function copyBuildInputTree() {
  const root = tempDir();
  const importers = [...SELECTED_IMPORTERS, ...PLATFORM_IMPORTERS];
  for (const importer of importers) {
    mkdirSync(join(root, importer), { recursive: true });
    copyFileSync(join(repoRoot, importer, "package.json"), join(root, importer, "package.json"));
    if (existsSync(join(repoRoot, importer, "tsconfig.json"))) {
      copyFileSync(
        join(repoRoot, importer, "tsconfig.json"),
        join(root, importer, "tsconfig.json"),
      );
    }
  }
  copyFileSync(join(repoRoot, "tsconfig.base.json"), join(root, "tsconfig.base.json"));
  return root;
}

describe("fixture lock: inclusion inventory", () => {
  it("selects the three roots plus linked importers and their peer-suffixed closure", () => {
    const result = computeLockFingerprint({
      lockText: fixtureLock,
      currentVersion: FIXTURE_VERSION,
    });
    expect(result.policyVersion).toBe(FINGERPRINT_POLICY_VERSION);
    expect(result.parser).toBe(FINGERPRINT_PARSER);
    expect(result.helperSha256).toMatch(/^[0-9a-f]{64}$/);
    expect(result.sha256).toMatch(/^[0-9a-f]{64}$/);
    expect(result.inventory.importers).toEqual([
      "packages/zfb",
      "packages/zfb-darwin-arm64",
      "packages/zfb-runtime",
      "packages/zfb-slugify",
    ]);
    expect(result.inventory.snapshots).toEqual([
      "@voidzero-dev/vite-plus-core@1.0.0(typescript@7.0.2)",
      "@voidzero-dev/vite-plus-darwin-arm64@1.0.0",
      "hono@4.12.25",
      "typescript@5.9.3",
      "typescript@7.0.2",
      "vite-plus@1.0.0(typescript@7.0.2)",
    ]);
    expect(result.inventory.packages).toEqual([
      "@voidzero-dev/vite-plus-core@1.0.0",
      "@voidzero-dev/vite-plus-darwin-arm64@1.0.0",
      "hono@4.12.25",
      "typescript@5.9.3",
      "typescript@7.0.2",
      "vite-plus@1.0.0",
    ]);
    expect(result.inventory.sections.project).toEqual([
      "catalogs",
      "importers",
      "lockfileVersion",
      "overrides",
      "packages",
      "settings",
      "snapshots",
    ]);
  });

  it("is deterministic and survives a parse/serialize round trip", () => {
    expect(fixtureHash(fixtureLock)).toBe(fixtureHash(fixtureLock));
    expectInvariant(
      fixtureLock,
      mutate(fixtureLock, () => {}),
      fixtureHash,
    );
  });
});

describe("invariance: content outside the inclusion set", () => {
  it("ignores a docs importer dependency bump", () => {
    expectInvariant(
      fixtureLock,
      mutate(fixtureLock, (_environment, project) => {
        project.importers.docs.dependencies.lodash = { specifier: "^4.17.22", version: "4.17.22" };
        addPackage(project, "lodash@4.17.22");
      }),
      fixtureHash,
    );
    expectInvariant(
      realLock,
      mutate(realLock, (_environment, project) => {
        project.importers.docs.dependencies.katex = { specifier: "0.16.46", version: "0.16.46" };
        addPackage(project, "katex@0.16.46", { dependencies: { commander: "8.3.0" } });
      }),
      realHash,
    );
  });

  it("ignores a root '.' devDependency addition and bump", () => {
    expectInvariant(
      realLock,
      mutate(realLock, (_environment, project) => {
        const root = project.importers["."].devDependencies;
        root["left-pad"] = { specifier: "1.3.0", version: "1.3.0" };
        addPackage(project, "left-pad@1.3.0");
        root.ws = { specifier: "^8.18.1", version: "8.18.1" };
        addPackage(project, "ws@8.18.1");
      }),
      realHash,
    );
  });

  it("ignores a mutation of the root-only peer variant of a shared package", () => {
    const mutated = mutate(realLock, (_environment, project) => {
      project.snapshots[ROOT_VITE_PLUS].transitivePeerDependencies.push("extra-peer");
    });
    expect(
      computeLockFingerprint({ lockText: realLock, currentVersion: REAL_VERSION }).inventory
        .snapshots,
    ).not.toContain(ROOT_VITE_PLUS);
    expectInvariant(realLock, mutated, realHash);
  });

  it("ignores key order, whitespace and comments", () => {
    const reorder = (document) => {
      for (const section of ["importers", "packages", "snapshots"]) {
        document[section] = Object.fromEntries(Object.entries(document[section]).reverse());
      }
    };
    expectInvariant(
      realLock,
      mutate(realLock, (environment, project) => {
        reorder(environment);
        reorder(project);
      }),
      realHash,
    );
    const decorated = realLock
      .replaceAll("\nimporters:\n", "\n# reviewer note\nimporters:\n\n")
      .replaceAll("\n\n  ", "\n\n\n  ")
      .replaceAll("lockfileVersion: '9.0'", 'lockfileVersion: "9.0"   # quoted differently');
    expectInvariant(realLock, decorated, realHash);
  });

  it("rebases the lockstep workspace specifier with the package version", () => {
    const bumped = realLock.replaceAll(`workspace:${REAL_VERSION}`, "workspace:99.0.0");
    expect(bumped).not.toBe(realLock);
    expectInvariant(realLock, bumped, realHash, { currentVersion: "99.0.0" });
    expectInvariant(
      fixtureLock,
      fixtureLock.replaceAll("workspace:1.0.0", "workspace:1.1.0"),
      fixtureHash,
      { currentVersion: "1.1.0" },
    );
    // Without the matching currentVersion the bumped specifier is not the lockstep one.
    expect(realHash(bumped)).not.toBe(realHash(realLock));
  });
});

describe("sensitivity: every included input moves the fingerprint", () => {
  it("hono version and integrity", () => {
    expectSensitive(
      realLock,
      mutate(realLock, (_environment, project) =>
        renameVersion(project, "hono", "4.12.25", "4.12.26"),
      ),
      realHash,
    );
    expectSensitive(
      realLock,
      mutate(realLock, (_environment, project) => {
        project.packages["hono@4.12.25"].resolution.integrity = "sha512-tampered==";
      }),
      realHash,
    );
  });

  it("typescript@7.0.2 to another version", () => {
    expectSensitive(
      realLock,
      mutate(realLock, (_environment, project) =>
        renameVersion(project, "typescript", "7.0.2", "7.0.3"),
      ),
      realHash,
    );
  });

  it("a transitive dependency reachable only through the packages' peer-suffixed vite-plus", () => {
    expect(
      computeLockFingerprint({ lockText: realLock, currentVersion: REAL_VERSION }).inventory
        .snapshots,
    ).toContain(PACKAGE_MOCKER);
    expectSensitive(
      realLock,
      mutate(realLock, (_environment, project) => {
        project.snapshots[PACKAGE_MOCKER].transitivePeerDependencies.push("extra-peer");
      }),
      realHash,
    );
  });

  it("an optional platform link", () => {
    const mutated = mutate(realLock, (_environment, project) => {
      delete project.importers["packages/zfb"].optionalDependencies["@takazudo/zfb-linux-x64-gnu"];
    });
    expectSensitive(realLock, mutated, realHash);
    expect(
      computeLockFingerprint({ lockText: mutated, currentVersion: REAL_VERSION }).inventory
        .importers,
    ).not.toContain("packages/zfb-linux-x64-gnu");
  });

  it("settings, overrides and catalogs", () => {
    const cases = [
      (project) => (project.settings.autoInstallPeers = false),
      (project) => (project.overrides.devalue = "^5.8.2"),
      (project) => (project.catalogs.default["vite-plus"].specifier = "1.0.1"),
    ];
    for (const change of cases) {
      expectSensitive(
        realLock,
        mutate(realLock, (_environment, project) => change(project)),
        realHash,
      );
    }
  });

  it("the environment document's pnpm version", () => {
    expectSensitive(
      realLock,
      mutate(realLock, (environment) => {
        environment.importers["."].packageManagerDependencies.pnpm.specifier = "12.8.3";
      }),
      realHash,
    );
  });

  it("a docs bump that introduces a dependency also reachable from a selected importer", () => {
    const introduce = (linkShared) =>
      mutate(realLock, (_environment, project) => {
        project.importers.docs.dependencies["shared-new"] = {
          specifier: "1.0.0",
          version: "1.0.0",
        };
        addPackage(project, "shared-new@1.0.0");
        if (linkShared) project.snapshots["hono@4.12.25"].dependencies = { "shared-new": "1.0.0" };
      });
    expectInvariant(realLock, introduce(false), realHash);
    expectSensitive(realLock, introduce(true), realHash);
  });

  it("does not rebase workspace:* like the lockstep specifier", () => {
    const edit = (specifier) =>
      mutate(realLock, (_environment, project) => {
        project.importers["packages/zfb-runtime"].devDependencies["@takazudo/zfb"].specifier =
          specifier;
      });
    expectSensitive(realLock, edit(`workspace:${REAL_VERSION}`), realHash);
    expectSensitive(realLock, edit("workspace:^"), realHash);
  });
});

describe("buildInputs: build recipes are pinned", () => {
  it("matches the repository tree when copied and ignores unrelated files", () => {
    const root = copyBuildInputTree();
    const baseline = realHash(realLock);
    expect(realHash(realLock, { repoRoot: root })).toBe(baseline);
    writeFileSync(join(root, "packages/zfb/README.md"), "unrelated\n");
    writeFileSync(join(root, "packages/zfb/tsconfig.zudo-react-fixture.json"), "{}\n");
    const manifestPath = join(root, "packages/zfb-runtime/package.json");
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    manifest.scripts.test = "vp test run --changed";
    writeFileSync(manifestPath, JSON.stringify(manifest));
    expect(realHash(realLock, { repoRoot: root })).toBe(baseline);
  });

  it("moves on a tsconfig, base tsconfig or build-script edit", () => {
    const baseline = realHash(realLock);
    const edits = [
      (root) => writeFileSync(join(root, "packages/zfb-slugify/tsconfig.json"), "{}\n"),
      (root) => writeFileSync(join(root, "tsconfig.base.json"), "{}\n"),
      (root) => {
        const path = join(root, "packages/zfb/package.json");
        const manifest = JSON.parse(readFileSync(path, "utf8"));
        manifest.scripts.build = "tsc -p tsconfig.build.json";
        writeFileSync(path, JSON.stringify(manifest));
      },
      (root) => {
        const path = join(root, "packages/zfb-darwin-arm64/package.json");
        const manifest = JSON.parse(readFileSync(path, "utf8"));
        manifest.scripts = { prepack: "node prepare-binary.mjs" };
        writeFileSync(path, JSON.stringify(manifest));
      },
    ];
    for (const edit of edits) {
      const root = copyBuildInputTree();
      edit(root);
      expect(realHash(realLock, { repoRoot: root })).not.toBe(baseline);
    }
  });

  it("fails closed when a selected importer has no package.json", () => {
    const root = copyBuildInputTree();
    rmSync(join(root, "packages/zfb-win32-x64-msvc/package.json"));
    expectFailure(() => realHash(realLock, { repoRoot: root }), "BUILD_INPUT");
  });
});

describe("fail-closed: malformed locks throw a named error", () => {
  const [environmentText, projectText] = fixtureLock.split(/^---\n/m).filter(Boolean);
  const join2 = (...documents) => documents.map((document) => `---\n${document}`).join("");
  const run = (lockText) => () => fixtureHash(lockText);

  it("duplicate keys in either document", () => {
    expectFailure(
      run(
        join2(
          environmentText.replace(
            "lockfileVersion: '9.0'\n",
            "lockfileVersion: '9.0'\nlockfileVersion: '9.0'\n",
          ),
          projectText,
        ),
      ),
      "YAML_PARSE",
    );
    expectFailure(
      run(
        join2(
          environmentText,
          projectText.replace(
            "  autoInstallPeers: true\n",
            "  autoInstallPeers: true\n  autoInstallPeers: false\n",
          ),
        ),
      ),
      "YAML_PARSE",
    );
  });

  it("one or three documents", () => {
    expectFailure(run(join2(environmentText)), "DOCUMENT_COUNT");
    expectFailure(
      run(join2(environmentText, projectText, "lockfileVersion: '9.0'\n")),
      "DOCUMENT_COUNT",
    );
  });

  it("swapped documents or an environment document without packageManagerDependencies", () => {
    expectFailure(run(join2(projectText, environmentText)), "UNKNOWN_KEY");
    expectFailure(
      run(
        mutate(fixtureLock, (environment) => {
          delete environment.importers["."].packageManagerDependencies;
        }),
      ),
      "ENVIRONMENT_DOCUMENT",
    );
  });

  it("a missing selected importer", () => {
    expectFailure(
      run(
        mutate(
          fixtureLock,
          (_environment, project) => delete project.importers["packages/zfb-runtime"],
        ),
      ),
      "MISSING_IMPORTER",
    );
  });

  it("a referenced snapshot or package record that is absent", () => {
    expectFailure(
      run(mutate(fixtureLock, (_environment, project) => delete project.snapshots["hono@4.12.25"])),
      "MISSING_SNAPSHOT",
    );
    expectFailure(
      run(mutate(fixtureLock, (_environment, project) => delete project.packages["hono@4.12.25"])),
      "MISSING_PACKAGE",
    );
    expectFailure(
      run(
        mutate(
          fixtureLock,
          (environment) => delete environment.snapshots["@pnpm/exe.darwin-arm64@12.8.2"],
        ),
      ),
      "MISSING_SNAPSHOT",
    );
  });

  it("link: to a missing importer, outside the workspace, or to the root", () => {
    const relink = (target) =>
      mutate(fixtureLock, (_environment, project) => {
        project.importers["packages/zfb"].dependencies["@takazudo/zfb-slugify"].version = target;
      });
    expectFailure(run(relink("link:../../outside")), "LINK_MISSING_IMPORTER");
    expectFailure(run(relink("link:../../../outside")), "LINK_ESCAPE");
    expectFailure(run(relink("link:/abs/zfb-slugify")), "LINK_ESCAPE");
    expectFailure(run(relink("link:../..")), "LINK_ROOT");
    expectFailure(run(relink("link:")), "MALFORMED_EDGE");
  });

  it("lockfileVersion other than '9.0' in either document", () => {
    expectFailure(
      run(fixtureLock.replace("lockfileVersion: '9.0'", "lockfileVersion: '6.0'")),
      "LOCKFILE_VERSION",
    );
    expectFailure(
      run(
        join2(
          environmentText,
          projectText.replace("lockfileVersion: '9.0'", "lockfileVersion: '10.0'"),
        ),
      ),
      "LOCKFILE_VERSION",
    );
  });

  it("a non-mapping importer", () => {
    expectFailure(
      run(fixtureLock.replace("packages/zfb-slugify: {}", "packages/zfb-slugify: oops")),
      "INVALID_SHAPE",
    );
  });

  it("unknown top-level keys in each document", () => {
    expectFailure(
      run(
        join2(environmentText.replace("importers:\n", "settings: {}\n\nimporters:\n"), projectText),
      ),
      "UNKNOWN_KEY",
    );
    expectFailure(
      run(join2(environmentText, `${projectText}\ntime:\n  hono@4.12.25: '2026-01-01'\n`)),
      "UNKNOWN_KEY",
    );
  });

  it("unknown keys inside records, even unreached ones", () => {
    const cases = [
      (project) => (project.packages["lodash@4.17.21"].requiresBuild = true),
      (project) =>
        (project.packages["hono@4.12.25"].resolution.tarball = "https://example.invalid/hono.tgz"),
      (project) => (project.snapshots["lodash@4.17.21"].patched = true),
      (project) => (project.importers.docs.dependencies.lodash.extra = "x"),
      (project) => (project.importers.docs.dependenciesMeta = {}),
    ];
    for (const change of cases) {
      expectFailure(
        run(mutate(fixtureLock, (_environment, project) => change(project))),
        "UNKNOWN_KEY",
      );
    }
  });

  it("YAML tags, anchors and aliases", () => {
    expectFailure(
      run(fixtureLock.replace("specifier: '>=4.12.25'", "specifier: !!str '>=4.12.25'")),
      "UNSUPPORTED_NODE",
    );
    expectFailure(
      run(
        fixtureLock
          .replace("  typescript@5.9.3: {}\n", "  typescript@5.9.3: &ts {}\n")
          .replace("  typescript@6.0.3: {}\n", "  typescript@6.0.3: *ts\n"),
      ),
      "UNSUPPORTED_NODE",
    );
  });

  it("non-string scalar leaves", () => {
    expectFailure(
      run(fixtureLock.replace("autoInstallPeers: true", "autoInstallPeers: 1")),
      "INVALID_SCALAR",
    );
    expectFailure(
      run(fixtureLock.replace("autoInstallPeers: true", "autoInstallPeers:")),
      "INVALID_SCALAR",
    );
  });

  it("malformed snapshot edges", () => {
    const edge = (value) =>
      mutate(fixtureLock, (_environment, project) => {
        project.snapshots["vite-plus@1.0.0(typescript@7.0.2)"].dependencies.vite = value;
      });
    expectFailure(
      run(edge("@voidzero-dev/vite-plus-core@1.0.0(typescript@7.0.2")),
      "MALFORMED_EDGE",
    );
    expectFailure(run(edge("@voidzero-dev/vite-plus-core@1.0.0)(")), "MALFORMED_EDGE");
    expectFailure(run(edge("link:../core")), "MALFORMED_EDGE");
    expectFailure(run(edge("file:../core")), "MALFORMED_EDGE");
    expectFailure(run(edge("")), "MALFORMED_EDGE");
  });

  it("a missing or empty currentVersion", () => {
    expectFailure(
      () => computeLockFingerprint({ lockText: fixtureLock, currentVersion: "" }),
      "INVALID_INPUT",
    );
  });
});

describe("helper identity", () => {
  it("hashes its own bytes and moves when the helper changes", () => {
    const digest = fingerprintHelperSha256();
    expect(digest).toMatch(/^[0-9a-f]{64}$/);
    expect(digest).toBe(sha(readFileSync(helperPath)));
    const copy = join(tempDir(), "lock-fingerprint.mjs");
    writeFileSync(copy, `${readFileSync(helperPath, "utf8")}// mutated\n`);
    expect(fingerprintHelperSha256(copy)).not.toBe(digest);
  });

  // #4198: a comment-only edit is helper maintenance, re-pinned by
  // `--reason fingerprint-helper`, because it cannot move the fingerprint.
  it("keeps the fingerprint and inventory when only a comment is added", async () => {
    const source = `${readFileSync(helperPath, "utf8")}// maintenance comment\n`;
    const current = computeLockFingerprint({ lockText: realLock, currentVersion: REAL_VERSION });
    const edited = await computeWithHelperSource(source, repoRoot, realLock, REAL_VERSION);
    expect(edited.sha256).toBe(current.sha256);
    expect(edited.inventory).toEqual(current.inventory);
    expect(edited.helperSha256).toBe(sha(source));
    expect(edited.helperSha256).not.toBe(current.helperSha256);
  });

  it("commits lock-fingerprint-inventory.json byte-for-byte as --explain prints it", () => {
    const explain = execFileSync(process.execPath, [helperPath, "--explain"], { encoding: "utf8" });
    expect(
      readFileSync(
        join(repoRoot, "research/v3-island-size/lock-fingerprint-inventory.json"),
        "utf8",
      ),
    ).toBe(explain);
  });
});

describe("real pnpm-lock.yaml", () => {
  it("fingerprints the real lock deterministically", () => {
    const first = computeLockFingerprint({ lockText: realLock, currentVersion: REAL_VERSION });
    const second = computeLockFingerprint({ lockText: realLock, currentVersion: REAL_VERSION });
    expect(second).toEqual(first);
    expect(first.inventory.snapshots).toEqual(
      expect.arrayContaining([
        "typescript@7.0.2",
        "typescript@6.0.3",
        "typescript@5.9.3",
        "hono@4.12.25",
        "yaml@2.9.0",
      ]),
    );
  });

  it("--explain lists exactly the three roots plus the five platform link importers", () => {
    const output = JSON.parse(
      execFileSync(process.execPath, [helperPath, "--explain"], { encoding: "utf8" }),
    );
    expect(output.sha256).toBe(realHash(realLock));
    expect(output.inventory.importers).toEqual(
      [...SELECTED_IMPORTERS, ...PLATFORM_IMPORTERS].sort(),
    );
  });
});
