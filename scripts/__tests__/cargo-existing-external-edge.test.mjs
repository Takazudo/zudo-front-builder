import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";
import {
  cargoRewireMarkdown,
  cargoRewireText,
  checkRewireDeclaration,
  proveCargoInternalRewire,
} from "../rebaseline-island-size-linux.mjs";
import {
  LINUX_TARGET,
  DARWIN_TARGET,
  REWIRE_MANIFESTS,
  recordOf,
  rewireSha as sha,
} from "./fixtures/island-size-cargo-rewire.mjs";
import {
  externalLocks,
  externalDeclaration,
  externalReceipt,
} from "./fixtures/island-size-external-edge.mjs";

function inputs(target = LINUX_TARGET) {
  const { oldText, newText, OLD, NEW } = externalLocks();
  return {
    contractSha: OLD,
    artifactSha: NEW,
    expectedOldSha: OLD,
    expectedNewSha: NEW,
    oldText,
    newText,
    oldLockSource: { commit: "a".repeat(40), path: "Cargo.lock", sha256: OLD },
    newLockSource: { commit: "b".repeat(40), path: "Cargo.lock", sha256: NEW },
    declaration: externalDeclaration(),
    declarationSource: { path: "review.json", sha256: sha(JSON.stringify(externalDeclaration())) },
    closures: Object.fromEntries(
      ["old", "new"].map((side) => {
        const receipt = externalReceipt(side, target);
        return [side, { receipt, sha256: sha(JSON.stringify(receipt)) }];
      }),
    ),
    target,
    readManifest: (path) => Buffer.from(REWIRE_MANIFESTS[path]),
  };
}
function prove(edit = () => {}, target) {
  const value = inputs(target);
  edit(value);
  return proveCargoInternalRewire(value);
}
// Rebind the lock and changed-record hashes so a negative tests the semantic
// refusal rather than an incidental stale digest. These are synthetic inputs.
function newLock(value, transform) {
  value.newText = transform(value.newText);
  const digest = sha(value.newText);
  value.expectedNewSha = value.artifactSha = value.newLockSource.sha256 = digest;
  value.declaration.newCargoLockSha256 = value.closures.new.receipt.cargoLockSha256 = digest;
  value.declaration.changed[0].newSha256 = sha(recordOf(value.newText, "zfb-render@0.0.0").body);
}
const row = (value, side, name = "sha2") =>
  value.closures[side].receipt.rows.find((r) => r.name === name);

describe("explicit existing-external-edge admission capability (not application)", () => {
  it("still refuses the same external edge under schema 1", () => {
    expect(() =>
      prove((v) => {
        v.declaration.schemaVersion = 1;
        delete v.declaration.externalEdges;
      }),
    ).toThrow(/not a declared internal record/);
  });

  it("refuses an ambiguous unversioned edge even when both external versions already exist", () => {
    expect(() =>
      prove((v) => {
        const extra =
          '\n[[package]]\nname = "sha2"\nversion = "0.11.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "' +
          "c".repeat(64) +
          '"\n';
        v.oldText += extra;
        v.contractSha = v.expectedOldSha = v.oldLockSource.sha256 = sha(v.oldText);
        v.declaration.oldCargoLockSha256 = v.closures.old.receipt.cargoLockSha256 = sha(v.oldText);
        newLock(v, (s) => s + extra);
      }),
    ).toThrow(/ambiguous/);
  });

  it.each([LINUX_TARGET, DARWIN_TARGET])(
    "proves exact declared edge and retains evidence for %s",
    (target) => {
      const result = prove(undefined, target);
      expect(result.externalEdges).toEqual(externalDeclaration().externalEdges);
      expect(result.added).toEqual([]);
      expect(result.removed).toEqual([]);
      expect(result.changed[0].addedDependencies).toEqual(["sha2"]);
      expect(result.closure).toMatchObject({
        target,
        added: [],
        changed: [],
        rows: { old: 6, new: 6, unchanged: 6 },
      });
    },
  );

  it("accepts a cargo program path that contains a space", () => {
    const result = prove((v) => {
      for (const side of ["old", "new"]) {
        v.closures[side].receipt.command[0] = "/Users/a b/.cargo/bin/cargo";
        v.closures[side].sha256 = sha(JSON.stringify(v.closures[side].receipt));
      }
    });
    expect(result.externalEdges).toHaveLength(1);
  });

  it("names the declared external edges in both transition summaries", () => {
    const result = prove();
    const [edge] = externalDeclaration().externalEdges;
    for (const summary of [cargoRewireText(result), cargoRewireMarkdown(result)]) {
      expect(summary).toContain("declared edges to unchanged external records");
      for (const value of [edge.from, edge.dependency, edge.to, edge.sha256]) {
        expect(summary).toContain(value);
      }
    }
  });

  it("permits an explicitly declared internal feature change while external features stay fixed", () => {
    const result = prove((v) => {
      row(v, "new", "zfb-render").features.push("new-internal-feature");
      v.declaration.closure.changed = ["zfb-render@0.0.0"];
    });
    expect(result.closure.changed).toEqual([
      {
        id: "zfb-render@0.0.0",
        from: ["default", "embed_v8"],
        to: ["default", "embed_v8", "new-internal-feature"],
      },
    ]);
  });

  it.each([
    [
      "schema 1 with an external exception",
      (d) => {
        d.schemaVersion = 1;
      },
      /externalEdges requires/,
    ],
    [
      "missing edge declarations",
      (d) => {
        delete d.externalEdges;
      },
      /nonempty externalEdges/,
    ],
    [
      "empty edge declarations",
      (d) => {
        d.externalEdges = [];
      },
      /nonempty externalEdges/,
    ],
    [
      "duplicate edge declaration",
      (d) => d.externalEdges.push({ ...d.externalEdges[0] }),
      /repeated external edge/,
    ],
    [
      "wrong source record",
      (d) => {
        d.externalEdges[0].from = "zfb-js-host@0.0.0";
      },
      /not an added dependency/,
    ],
    [
      "unused declaration",
      (d) => {
        d.externalEdges[0].dependency = "anyhow";
      },
      /not an added dependency/,
    ],
    [
      "unrelated undeclared edge",
      (d) => d.changed[0].addedDependencies.push("anyhow"),
      /undeclared external edge/,
    ],
    [
      "unknown edge key",
      (d) => {
        d.externalEdges[0].allow = true;
      },
      /exactly from/,
    ],
    [
      "missing target hash",
      (d) => {
        delete d.externalEdges[0].sha256;
      },
      /exactly from/,
    ],
    [
      "removed dependency",
      (d) => d.changed[0].removedDependencies.push("anyhow"),
      /only added external edges/,
    ],
    [
      "added internal record",
      (d) =>
        d.added.push({
          id: "x@0.0.0",
          manifest: "crates/x/Cargo.toml",
          sha256: "a".repeat(64),
          dependencies: [],
        }),
      /only added external edges/,
    ],
    [
      "closure addition",
      (d) => d.closure.added.push("zfb-render@0.0.0"),
      /only added external edges/,
    ],
  ])("rejects %s", (_name, edit, error) => {
    expect(() => checkRewireDeclaration(externalDeclaration(edit))).toThrow(error);
  });

  it.each([
    [
      "wrong external target",
      (v) => {
        v.declaration.externalEdges[0].to = "anyhow@1.0.100";
      },
      /target identity or bytes differ/,
    ],
    [
      "wrong external record hash",
      (v) => {
        v.declaration.externalEdges[0].sha256 = "f".repeat(64);
      },
      /target identity or bytes differ/,
    ],
    ["external feature drift", (v) => row(v, "new").features.push("asm"), /external feature drift/],
    [
      "unrelated external feature drift",
      (v) => row(v, "new", "anyhow").features.push("extra"),
      /external feature drift/,
    ],
    [
      "external source drift in receipt",
      (v) => {
        row(v, "new").source = "registry+https://example.invalid";
      },
      /source differs/,
    ],
    [
      "external version drift",
      (v) => newLock(v, (s) => s.replace('version = "0.10.9"', 'version = "0.11.0"')),
      /removed records sha2/,
    ],
    [
      "external source drift in lock",
      (v) =>
        newLock(v, (s) =>
          s.replaceAll(
            "registry+https://github.com/rust-lang/crates.io-index",
            "registry+https://example.invalid",
          ),
        ),
      /undeclared changed records/,
    ],
    [
      "external checksum drift",
      (v) => newLock(v, (s) => s.replace('checksum = "a750', 'checksum = "b750')),
      /undeclared changed records/,
    ],
    [
      "new external package",
      (v) =>
        newLock(
          v,
          (s) =>
            s +
            '\n[[package]]\nname = "new-external"\nversion = "1.0.0"\nsource = "registry+https://example.invalid"\n',
        ),
      /undeclared added records/,
    ],
    [
      "external record removal",
      (v) => newLock(v, (s) => s.replace(recordOf(s, "anyhow@1.0.100").body, "")),
      /removed records anyhow/,
    ],
    [
      "unrelated internal edge",
      (v) =>
        newLock(v, (s) =>
          s.replace(
            'name = "zudo-wind"\nversion = "0.0.0"\ndependencies = [',
            'name = "zudo-wind"\nversion = "0.0.0"\ndependencies = [\n "anyhow",',
          ),
        ),
      /undeclared changed records zudo-wind/,
    ],
    [
      "undeclared internal feature drift",
      (v) => row(v, "new", "zfb-render").features.push("extra"),
      /differ from the declaration/,
    ],
    [
      "target absent from old closure",
      (v) => {
        v.closures.old.receipt.rows = v.closures.old.receipt.rows.filter((r) => r.name !== "sha2");
      },
      /must already contain both endpoints/,
    ],
    [
      "target omitted from both receipts",
      (v) => {
        for (const side of ["old", "new"])
          v.closures[side].receipt.rows = v.closures[side].receipt.rows.filter(
            (r) => r.name !== "sha2",
          );
      },
      /must already contain both endpoints/,
    ],
    [
      "internal source absent from old closure",
      (v) => {
        v.closures.old.receipt.rows = v.closures.old.receipt.rows.filter(
          (r) => r.name !== "zfb-render",
        );
      },
      /must already contain both endpoints/,
    ],
    [
      "wrong platform",
      (v) => {
        v.closures.old.receipt.target = DARWIN_TARGET;
      },
      /recorded for target/,
    ],
    [
      "wrong producer",
      (v) => {
        v.closures.new.receipt.producer = "zudo-wind";
      },
      /recorded for producer/,
    ],
    [
      "wrong lock receipt",
      (v) => {
        v.closures.new.receipt.cargoLockSha256 = "f".repeat(64);
      },
      /recorded at Cargo.lock/,
    ],
    [
      "missing closure",
      (v) => {
        delete v.closures.new;
      },
      /missing or has no sha256/,
    ],
    [
      "missing closure digest",
      (v) => {
        delete v.closures.old.sha256;
      },
      /missing or has no sha256/,
    ],
    [
      "missing declaration source",
      (v) => {
        delete v.declarationSource;
      },
      /declaration source/,
    ],
    [
      "wrong artifact lock",
      (v) => {
        v.artifactSha = v.expectedOldSha;
      },
      /artifact measured Cargo.lock/,
    ],
    [
      "wrong old source digest",
      (v) => {
        v.oldLockSource.sha256 = "f".repeat(64);
      },
      /verified git blob/,
    ],
    [
      "wrong new source digest",
      (v) => {
        v.newLockSource.sha256 = "f".repeat(64);
      },
      /new Cargo.lock source digest/,
    ],
    [
      "nondefault feature selection",
      (v) => v.closures.new.receipt.command.push("--no-default-features"),
      /default-feature metadata command/,
    ],
    [
      "flags folded into the program name",
      (v) => {
        v.closures.new.receipt.command[0] = "cargo --no-default-features";
      },
      /default-feature metadata command/,
    ],
    [
      "toolchain override folded into the program name",
      (v) => {
        v.closures.old.receipt.command[0] = "cargo +nightly";
      },
      /default-feature metadata command/,
    ],
    [
      "missing command attestation",
      (v) => {
        delete v.closures.old.receipt.command;
      },
      /default-feature metadata command/,
    ],
    [
      "producer default feature omitted",
      (v) => {
        row(v, "old", "zfb").features = [];
      },
      /producer's default feature/,
    ],
    [
      "duplicate literal dependency",
      (v) =>
        newLock(v, (s) =>
          s.replace(' "sha2",\n "zfb-js-host",', ' "sha2",\n "sha2",\n "zfb-js-host",'),
        ),
      /repeats a dependency entry/,
    ],
    [
      "dev edges",
      (v) => {
        v.closures.old.receipt.edges = "all";
      },
      /normal,build/,
    ],
  ])("refuses %s even with explicit schema 2", (_name, edit, error) => {
    expect(() => prove(edit)).toThrow(error);
  });

  it("does not accept an internal target as external", () => {
    expect(() =>
      prove((v) => {
        newLock(v, (s) => s.replace(' "sha2",\n "zfb-js-host",', ' "zudo-wind",\n "zfb-js-host",'));
        v.declaration.changed[0].addedDependencies = ["zudo-wind"];
        v.declaration.externalEdges[0] = {
          from: "zfb-render@0.0.0",
          dependency: "zudo-wind",
          to: "zudo-wind@0.0.0",
          sha256: sha(recordOf(v.newText, "zudo-wind@0.0.0").body),
        };
      }),
    ).toThrow(/existing byte-identical external record/);
  });

  it("rejects a differently spelled duplicate of an existing dependency", () => {
    expect(() =>
      prove((v) => {
        newLock(v, (s) =>
          s.replace(' "sha2",\n "zfb-js-host",', ' "anyhow 1.0.100",\n "zfb-js-host",'),
        );
        v.declaration.changed[0].addedDependencies = ["anyhow 1.0.100"];
        v.declaration.externalEdges[0] = {
          from: "zfb-render@0.0.0",
          dependency: "anyhow 1.0.100",
          to: "anyhow@1.0.100",
          sha256: sha(recordOf(v.newText, "anyhow@1.0.100").body),
        };
      }),
    ).toThrow(/already existed under another dependency spelling/);
  });

  it("rejects two newly added spellings of the same external target", () => {
    expect(() =>
      prove((v) => {
        newLock(v, (s) =>
          s.replace(' "sha2",\n "zfb-js-host",', ' "sha2",\n "sha2 0.10.9",\n "zfb-js-host",'),
        );
        v.declaration.changed[0].addedDependencies.push("sha2 0.10.9");
        v.declaration.externalEdges.push({
          ...v.declaration.externalEdges[0],
          dependency: "sha2 0.10.9",
        });
      }),
    ).toThrow(/repeats an added external target/);
  });
});

describe("committed #4429 oxc edge declaration (rolldown-oxc-4429.json)", () => {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
  const dir = join(root, "research/v3-island-size/cargo-transitions");

  it("proves the declaration against the frozen old lock and the live Cargo.lock for both targets", () => {
    const oldText = readFileSync(join(dir, "rolldown-oxc-4429.old.lock"), "utf8");
    const newText = readFileSync(join(root, "Cargo.lock"), "utf8");
    const declarationBytes = readFileSync(join(dir, "rolldown-oxc-4429.json"));
    for (const target of [LINUX_TARGET, DARWIN_TARGET]) {
      const receipt = (side) => {
        const bytes = readFileSync(join(dir, `rolldown-oxc-4429/closure-${side}-${target}.json`));
        return { receipt: JSON.parse(bytes), sha256: sha(bytes) };
      };
      const proof = proveCargoInternalRewire({
        contractSha: sha(oldText),
        artifactSha: sha(newText),
        expectedOldSha: sha(oldText),
        expectedNewSha: sha(newText),
        oldText,
        newText,
        oldLockSource: {
          commit: "7b062415c002151adf20b52aaf77019a90adfa0e",
          path: "Cargo.lock",
          sha256: sha(oldText),
        },
        newLockSource: { sha256: sha(newText) },
        declaration: JSON.parse(declarationBytes),
        declarationSource: { path: "rolldown-oxc-4429.json", sha256: sha(declarationBytes) },
        closures: { old: receipt("old"), new: receipt("new") },
        target,
        readManifest: (path) => readFileSync(join(root, path)),
      });
      expect(proof.externalEdges).toEqual([
        expect.objectContaining({ from: "zfb-rolldown-prototype@0.0.0", to: "oxc@0.153.0" }),
      ]);
    }
  });
});
