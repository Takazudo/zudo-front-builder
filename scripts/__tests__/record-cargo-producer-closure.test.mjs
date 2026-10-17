import { describe, expect, it } from "vite-plus/test";

import {
  CLOSURE_EDGES,
  CLOSURE_RECEIPT_SCHEMA,
  closureFromMetadata,
  metadataCommand,
  normalizeSource,
  parseArgs,
} from "../record-cargo-producer-closure.mjs";

// Test fixture — a synthetic `cargo metadata` document, not a real resolve:
// the producer `zfb` reaches `zfb-render` (normal), `build-dep` (build only)
// and, through zfb-render, the registry crate `serde`; `dev-only` hangs off a
// dev edge and must not be selected; `orphan` is unreachable.
const ROOT = "/checkout";
const REGISTRY = "registry+https://github.com/rust-lang/crates.io-index";
const id = (name, version, source) => source + "#" + name + "@" + version;
const ZFB = "path+file:///checkout/crates/zfb#0.0.0";
const RENDER = "path+file:///checkout/crates/zfb-render#0.0.0";
const SERDE = id("serde", "1.0.219", REGISTRY);
const BUILD = id("build-dep", "0.1.0", REGISTRY);
const DEV = id("dev-only", "2.0.0", REGISTRY);
const ORPHAN = "path+file:///checkout/crates/orphan#0.0.0";
const metadata = {
  workspace_members: [ZFB, RENDER, ORPHAN],
  packages: [
    { id: ZFB, name: "zfb", version: "0.0.0" },
    { id: RENDER, name: "zfb-render", version: "0.0.0" },
    { id: SERDE, name: "serde", version: "1.0.219" },
    { id: BUILD, name: "build-dep", version: "0.1.0" },
    { id: DEV, name: "dev-only", version: "2.0.0" },
    { id: ORPHAN, name: "orphan", version: "0.0.0" },
  ],
  resolve: {
    nodes: [
      {
        id: ZFB,
        features: ["default", "embed_v8"],
        deps: [
          { pkg: RENDER, dep_kinds: [{ kind: null, target: null }] },
          { pkg: BUILD, dep_kinds: [{ kind: "build", target: null }] },
          { pkg: DEV, dep_kinds: [{ kind: "dev", target: null }] },
        ],
      },
      {
        id: RENDER,
        features: ["std", "default"],
        deps: [
          {
            pkg: SERDE,
            dep_kinds: [
              { kind: null, target: null },
              { kind: "dev", target: null },
            ],
          },
        ],
      },
      { id: SERDE, features: ["derive", "default", "derive"], deps: [] },
      { id: BUILD, features: [], deps: [] },
      { id: DEV, features: ["x"], deps: [] },
      { id: ORPHAN, features: [], deps: [] },
    ],
  },
};

describe("record-cargo-producer-closure", () => {
  it("walks normal and build edges from the producer, never dev-only edges", () => {
    expect(closureFromMetadata(metadata, { producer: "zfb", root: ROOT })).toEqual([
      { name: "build-dep", version: "0.1.0", source: REGISTRY, features: [] },
      { name: "serde", version: "1.0.219", source: REGISTRY, features: ["default", "derive"] },
      {
        name: "zfb",
        version: "0.0.0",
        source: "path+crates/zfb",
        features: ["default", "embed_v8"],
      },
      {
        name: "zfb-render",
        version: "0.0.0",
        source: "path+crates/zfb-render",
        features: ["default", "std"],
      },
    ]);
  });

  it("requires exactly one workspace member named like the producer", () => {
    expect(() => closureFromMetadata(metadata, { producer: "serde", root: ROOT })).toThrow(
      "producer serde must be exactly one workspace member, found 0",
    );
    expect(() => closureFromMetadata(metadata, { producer: "nope", root: ROOT })).toThrow(
      /found 0/,
    );
  });

  it("records path sources relative to the checkout and refuses one outside it", () => {
    expect(normalizeSource("path+file:///checkout/crates/zfb#0.0.0", ROOT)).toBe("path+crates/zfb");
    expect(normalizeSource("path+file:///checkout#0.0.0", ROOT)).toBe("path+.");
    expect(normalizeSource(SERDE, ROOT)).toBe(REGISTRY);
    expect(normalizeSource("git+https://example.invalid/x?rev=1#abc", ROOT)).toBe(
      "git+https://example.invalid/x?rev=1",
    );
    expect(() => normalizeSource("path+file:///elsewhere/x#0.0.0", ROOT)).toThrow(
      /lies outside the checkout root/,
    );
  });

  it("pins the receipt contract and validates its arguments", () => {
    expect(CLOSURE_RECEIPT_SCHEMA).toBe(1);
    expect(CLOSURE_EDGES).toBe("normal,build");
    expect(metadataCommand("aarch64-apple-darwin")).toEqual([
      "metadata",
      "--locked",
      "--format-version",
      "1",
      "--filter-platform",
      "aarch64-apple-darwin",
    ]);
    expect(
      parseArgs([
        "--producer",
        "zfb",
        "--target",
        "x86_64-unknown-linux-gnu",
        "--output",
        "o.json",
      ]),
    ).toEqual({
      cargo: "cargo",
      root: ".",
      producer: "zfb",
      target: "x86_64-unknown-linux-gnu",
      output: "o.json",
    });
    expect(() => parseArgs(["--producer", "zfb"])).toThrow("required --target");
    expect(() =>
      parseArgs(["--producer", "zfb", "--target", "t", "--output", "o", "--x", "1"]),
    ).toThrow("unknown argument --x");
    expect(() => parseArgs(["--producer"])).toThrow("bad argument: --producer");
  });
});
