// Tests for scripts/check-provenance-preflight.mjs.
//
// This is the PRE-publish half of the provenance guard (check-provenance-drift.mjs
// is the after-the-fact half): given a version about to be published, would
// publishing it regress an npm consumer's trust (pnpm trustPolicy=no-downgrade)?
// Everything here runs offline against hand-built packuments and an injected
// fetch, matching check-provenance-drift.test.mjs's approach.

import { afterEach, describe, expect, it } from "vite-plus/test";

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  DEFAULT_POLICY_PATH,
  applyPolicy,
  checkMode,
  checkPackage,
  loadPolicy,
  parseArgs,
  report,
  wouldDowngrade,
} from "../check-provenance-preflight.mjs";

/** Build a packument. `versions` maps version → {attested, time}. */
function packument(versions) {
  return {
    time: Object.fromEntries(Object.entries(versions).map(([v, spec]) => [v, spec.time])),
    versions: Object.fromEntries(
      Object.entries(versions).map(([v, spec]) => [
        v,
        { dist: spec.attested ? { attestations: { url: "x" } } : {} },
      ]),
    ),
  };
}

function notFound(name) {
  const error = new Error(`${name}: registry responded 404 Not Found`);
  error.status = 404;
  return error;
}

describe("wouldDowngrade", () => {
  it("flags a target that would regress a stable version attested earlier", () => {
    const result = wouldDowngrade(
      packument({
        "2.19.0": { attested: true, time: "2026-08-01T00:00:00.000Z" },
        "2.20.2": { attested: true, time: "2026-08-20T00:00:00.000Z" },
      }),
      { version: "2.20.3" },
    );
    expect(result).toBe("2.19.0");
  });

  it("is ok when no other version was ever attested", () => {
    const result = wouldDowngrade(
      packument({
        "2.19.0": { attested: false, time: "2026-08-01T00:00:00.000Z" },
        "2.20.2": { attested: false, time: "2026-08-20T00:00:00.000Z" },
      }),
      { version: "2.20.3" },
    );
    expect(result).toBeNull();
  });

  it("ignores a prerelease attestation when the target is stable", () => {
    // Matches pnpm >= 10.24 and check-provenance-drift.mjs's classifyPackage:
    // a prerelease's attestation must not raise a downgrade against a stable
    // release pnpm itself would not flag.
    const result = wouldDowngrade(
      packument({
        "2.20.3-next.1": { attested: true, time: "2026-08-01T00:00:00.000Z" },
      }),
      { version: "2.20.3" },
    );
    expect(result).toBeNull();
  });

  it("counts a prerelease attestation when the target is itself a prerelease", () => {
    const result = wouldDowngrade(
      packument({
        "2.20.3-next.1": { attested: true, time: "2026-08-01T00:00:00.000Z" },
      }),
      { version: "2.20.3-next.2" },
    );
    expect(result).toBe("2.20.3-next.1");
  });

  it("reports the earliest attested version, not merely any of them", () => {
    const result = wouldDowngrade(
      packument({
        "2.18.0": { attested: true, time: "2026-06-01T00:00:00.000Z" },
        "2.19.0": { attested: true, time: "2026-07-01T00:00:00.000Z" },
        "2.20.2": { attested: true, time: "2026-08-20T00:00:00.000Z" },
      }),
      { version: "2.20.3" },
    );
    expect(result).toBe("2.18.0");
  });

  it("never treats the target version itself as prior evidence", () => {
    // If the target version were somehow already attested in the packument
    // (should never happen — checkPackage skips it before calling this), it
    // must not count against itself.
    const result = wouldDowngrade(
      packument({
        "2.20.3": { attested: true, time: "2026-08-01T00:00:00.000Z" },
      }),
      { version: "2.20.3" },
    );
    expect(result).toBeNull();
  });
});

describe("checkPackage", () => {
  it("returns a downgrade verdict for a version that would regress trust", async () => {
    const result = await checkPackage("@takazudo/zfb-darwin-x64", {
      version: "2.20.3",
      fetchOne: async () =>
        packument({
          "2.19.0": { attested: true, time: "2026-08-01T00:00:00.000Z" },
        }),
    });
    expect(result).toEqual({
      name: "@takazudo/zfb-darwin-x64",
      status: "downgrade",
      version: "2.20.3",
      earlierAttested: "2.19.0",
    });
  });

  it("returns ok when the package has never carried an attestation", async () => {
    const result = await checkPackage("pkg", {
      version: "2.20.3",
      fetchOne: async () =>
        packument({
          "2.19.0": { attested: false, time: "2026-08-01T00:00:00.000Z" },
        }),
    });
    expect(result).toEqual({ name: "pkg", status: "ok" });
  });

  it("skips a package that already has the target version published", async () => {
    // The publish step itself skips already-published versions (release.yml
    // ~1052) — a retried or partial publish must not false-fail here.
    const result = await checkPackage("pkg", {
      version: "2.20.3",
      fetchOne: async () =>
        packument({
          "2.20.3": { attested: false, time: "2026-08-27T00:00:00.000Z" },
          "2.19.0": { attested: true, time: "2026-08-01T00:00:00.000Z" },
        }),
    });
    expect(result).toEqual({
      name: "pkg",
      status: "skipped",
      detail: "2.20.3 is already published",
    });
  });

  it("treats a 404 (never published) as ok", async () => {
    const result = await checkPackage("brand-new-pkg", {
      version: "1.0.0",
      fetchOne: async () => {
        throw notFound("brand-new-pkg");
      },
    });
    expect(result).toEqual({
      name: "brand-new-pkg",
      status: "ok",
      detail: "package has never been published",
    });
  });

  it("rethrows a non-404 fetch failure so the caller can fail closed", async () => {
    await expect(
      checkPackage("pkg", {
        version: "1.0.0",
        fetchOne: async () => {
          throw new Error("pkg: registry responded 503 Service Unavailable");
        },
      }),
    ).rejects.toThrow(/503/);
  });
});

describe("checkMode", () => {
  it("maps mac-local to both owner-Mac Darwin carriers (#4690)", async () => {
    const seen = [];
    const results = await checkMode("mac-local", {
      version: "1.0.0",
      fetchOne: async (name) => {
        seen.push(name);
        return packument({});
      },
    });
    expect(seen.sort()).toEqual(["@takazudo/zfb-darwin-arm64", "@takazudo/zfb-darwin-x64"]);
    expect(results.every((r) => r.status === "ok")).toBe(true);
  });

  it("flags both Darwin carriers as a downgrade after an all-attested release like 6.0.1", async () => {
    // 6.0.1 was CI-built with provenance for every package; the first release
    // whose Darwin carriers come from the owner's Mac (--provenance=false) is
    // a downgrade for both, which the default-deny gate must block.
    const results = await checkMode("mac-local", {
      version: "6.0.2",
      fetchOne: async () =>
        packument({ "6.0.1": { attested: true, time: "2026-11-09T00:00:00.000Z" } }),
    });
    expect(results.map((r) => [r.name, r.status]).sort()).toEqual([
      ["@takazudo/zfb-darwin-arm64", "downgrade"],
      ["@takazudo/zfb-darwin-x64", "downgrade"],
    ]);
    expect(report(results, { log: () => {} })).toBe(1);
  });

  it("honors an explicit --package list for mac-local", async () => {
    const seen = [];
    await checkMode("mac-local", {
      version: "1.0.0",
      packages: ["@takazudo/zfb-darwin-x64"],
      fetchOne: async (name) => {
        seen.push(name);
        return packument({});
      },
    });
    expect(seen).toEqual(["@takazudo/zfb-darwin-x64"]);
  });

  it("maps recovery-no-provenance to all 11 published packages", async () => {
    const seen = [];
    const results = await checkMode("recovery-no-provenance", {
      version: "1.0.0",
      fetchOne: async (name) => {
        seen.push(name);
        return packument({});
      },
    });
    expect(seen).toHaveLength(11);
    expect(seen).toContain("@takazudo/zfb-slugify");
    expect(seen).toContain("@takazudo/zfb");
    expect(seen).toContain("@takazudo/zfb-darwin-x64");
    expect(results).toHaveLength(11);
    expect(results.every((r) => r.status === "ok")).toBe(true);
  });

  it("rejects an unknown mode", async () => {
    await expect(checkMode("bogus", { version: "1.0.0" })).rejects.toThrow(/unknown --mode/);
  });

  it("turns a per-package registry failure into an error verdict rather than rejecting", async () => {
    const results = await checkMode("recovery-no-provenance", {
      version: "1.0.0",
      fetchOne: async (name) => {
        if (name === "@takazudo/zfb") {
          throw new Error("@takazudo/zfb: registry responded 500 Internal Server Error");
        }
        return packument({});
      },
    });
    const failed = results.find((r) => r.name === "@takazudo/zfb");
    expect(failed.status).toBe("error");
    expect(failed.detail).toMatch(/500/);
  });
});

describe("report", () => {
  const lines = () => {
    const out = [];
    return { out, log: (m) => out.push(m) };
  };

  it("exits 0 when nothing would downgrade", () => {
    const { out, log } = lines();
    expect(report([{ name: "pkg", status: "ok" }], { log })).toBe(0);
    expect(out.some((l) => l.startsWith("DOWNGRADE"))).toBe(false);
  });

  it("exits 1 and prints one DOWNGRADE line per regressed package", () => {
    const { out, log } = lines();
    const code = report(
      [{ name: "pkg", status: "downgrade", version: "2.20.3", earlierAttested: "2.19.0" }],
      { log },
    );
    expect(code).toBe(1);
    expect(out).toContain("DOWNGRADE pkg@2.20.3 (earlier attested: 2.19.0)");
  });

  it("exits 2 when any package could not be checked, even if none would downgrade", () => {
    // A registry error means the picture is incomplete — fail closed rather
    // than reporting a clean pass built on partial data.
    const { out, log } = lines();
    const code = report(
      [
        { name: "ok-pkg", status: "ok" },
        { name: "broken-pkg", status: "error", detail: "boom" },
      ],
      { log },
    );
    expect(code).toBe(2);
    expect(out.some((l) => l.includes("broken-pkg"))).toBe(true);
  });

  it("exits 2 over exit 1 when a run has both an error and a downgrade", () => {
    const { log } = lines();
    const code = report(
      [
        { name: "a", status: "downgrade", version: "1.0.0", earlierAttested: "0.9.0" },
        { name: "b", status: "error", detail: "boom" },
      ],
      { log },
    );
    expect(code).toBe(2);
  });
});

describe("parseArgs", () => {
  it("parses --version and --mode in either order", () => {
    expect(parseArgs(["--version", "2.20.3", "--mode", "mac-local"])).toEqual({
      version: "2.20.3",
      mode: "mac-local",
    });
    expect(parseArgs(["--mode", "recovery-no-provenance", "--version", "2.20.3"])).toEqual({
      version: "2.20.3",
      mode: "recovery-no-provenance",
    });
  });

  it("collects --package for mac-local only, and only published names", () => {
    expect(
      parseArgs([
        "--version",
        "6.0.2",
        "--mode",
        "mac-local",
        "--package",
        "@takazudo/zfb-darwin-arm64",
        "--package",
        "@takazudo/zfb-darwin-x64",
      ]),
    ).toEqual({
      version: "6.0.2",
      mode: "mac-local",
      packages: ["@takazudo/zfb-darwin-arm64", "@takazudo/zfb-darwin-x64"],
    });
    expect(
      parseArgs(["--version", "6.0.2", "--mode", "mac-local", "--package", "left-pad"]),
    ).toBeNull();
    expect(
      parseArgs([
        "--version",
        "6.0.2",
        "--mode",
        "recovery-no-provenance",
        "--package",
        "@takazudo/zfb",
      ]),
    ).toBeNull();
    expect(parseArgs(["--version", "6.0.2", "--mode", "mac-local", "--bogus"])).toBeNull();
  });

  it("returns null when a required flag is missing", () => {
    expect(parseArgs(["--version", "2.20.3"])).toBeNull();
    expect(parseArgs(["--mode", "mac-local"])).toBeNull();
    expect(parseArgs([])).toBeNull();
  });
});

describe("owner provenance policy (release/provenance-policy.json, #4690)", () => {
  const ARM = "@takazudo/zfb-darwin-arm64";
  const X64 = "@takazudo/zfb-darwin-x64";
  const decision = (pkg, from = "6.1.0") => ({
    package: pkg,
    unattested_from_version: from,
    reason: "Darwin carriers are built on the owner's Mac",
    decided_by: "owner",
    issue: "https://github.com/Takazudo/zudo-front-builder/issues/4690",
  });
  const policy = (...decisions) => ({ schema: "zfb-provenance-policy/v1", decisions });
  const down = (name, version = "6.1.0") => ({
    name,
    status: "downgrade",
    version,
    earlierAttested: "6.0.1",
  });

  it("is read from the control checkout's release/ directory and absent by default", () => {
    expect(DEFAULT_POLICY_PATH.endsWith(join("release", "provenance-policy.json"))).toBe(true);
    expect(loadPolicy(join(tmpdir(), "zfb-no-such-policy.json"))).toBeNull();
  });

  it("acknowledges a listed local-built carrier at or after its version, naming the decision", () => {
    const out = [];
    const results = applyPolicy([down(ARM), down(X64)], {
      mode: "mac-local",
      version: "6.1.0",
      policy: policy(decision(ARM), decision(X64)),
      localBuilt: [ARM, X64],
    });
    expect(results.map((r) => r.status)).toEqual(["policy-acknowledged", "policy-acknowledged"]);
    expect(report(results, { log: (l) => out.push(l) })).toBe(0);
    expect(out.join("\n")).toContain(
      `ACKNOWLEDGED ${ARM}@6.1.0 (earlier attested: 6.0.1) by release/provenance-policy.json: unattested_from_version=6.1.0 decided_by=owner`,
    );
  });

  it.each([
    ["a version before unattested_from_version", { version: "6.0.2" }],
    ["recovery mode", { mode: "recovery-no-provenance" }],
    ["a package that is not local-built in this run", { localBuilt: [X64] }],
    ["an unlisted package", { policy: policy(decision(X64)) }],
    ["no policy at all", { policy: null }],
  ])("still blocks for %s", (_label, overrides) => {
    const ctx = {
      mode: "mac-local",
      version: "6.1.0",
      policy: policy(decision(ARM)),
      localBuilt: [ARM, X64],
      ...overrides,
    };
    const results = applyPolicy([down(ARM, ctx.version)], ctx);
    expect(results[0].status).toBe("downgrade");
    expect(report(results, { log: () => {} })).toBe(1);
  });

  it("never covers a CI-built package even when listed", () => {
    const results = applyPolicy([down("@takazudo/zfb")], {
      mode: "mac-local",
      version: "6.1.0",
      policy: policy(decision("@takazudo/zfb")),
      localBuilt: [ARM, X64],
    });
    expect(results[0].status).toBe("downgrade");
  });

  it("acknowledges a later prerelease/stable but not an earlier prerelease of the same core", () => {
    const ctx = { mode: "mac-local", policy: policy(decision(ARM, "6.1.0")), localBuilt: [ARM] };
    expect(
      applyPolicy([down(ARM, "6.2.0-next.1")], { ...ctx, version: "6.2.0-next.1" })[0].status,
    ).toBe("policy-acknowledged");
    expect(
      applyPolicy([down(ARM, "6.1.0-next.3")], { ...ctx, version: "6.1.0-next.3" })[0].status,
    ).toBe("downgrade");
  });

  describe("loadPolicy validation (fail closed)", () => {
    let dir;
    const write = (body) => {
      dir = mkdtempSync(join(tmpdir(), "zfb-policy-"));
      const path = join(dir, "provenance-policy.json");
      writeFileSync(path, typeof body === "string" ? body : JSON.stringify(body));
      return path;
    };
    afterEach(() => dir && rmSync(dir, { recursive: true, force: true }));

    it("accepts a well-formed policy", () => {
      expect(loadPolicy(write(policy(decision(ARM)))).decisions).toHaveLength(1);
    });
    it.each([
      ["malformed JSON", "{nope"],
      ["a wrong schema", { schema: "x", decisions: [] }],
      ["a missing reason", policy({ ...decision(ARM), reason: "" })],
      ["an unknown package", policy(decision("left-pad"))],
      ["a bad version", policy(decision(ARM, "six"))],
      ["a duplicate package", policy(decision(ARM), decision(ARM, "7.0.0"))],
    ])("rejects %s", (_label, body) => {
      expect(() => loadPolicy(write(body))).toThrow();
    });
  });

  it("parses --policy", () => {
    expect(
      parseArgs(["--version", "6.1.0", "--mode", "mac-local", "--policy", "/p.json"]),
    ).toMatchObject({
      policy: "/p.json",
    });
    expect(parseArgs(["--version", "6.1.0", "--mode", "mac-local", "--policy"])).toBeNull();
  });
});
