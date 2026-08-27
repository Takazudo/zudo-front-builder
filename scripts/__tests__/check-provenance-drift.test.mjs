// Tests for scripts/check-provenance-drift.mjs.
//
// The whole value of this check is the DISCRIMINATION it makes: a package that
// LOST an attestation must fail, while one that never had it must only warn.
// Collapse those two and the check is worthless in one direction or the other —
// it either misses the v2.12.0 regression (#2623) or goes red every week over
// zfb-darwin-x64's standing gap (#2625) until someone mutes it. So the bulk of
// these tests pin that boundary and the two pnpm-matching rules that decide
// which side of it a package falls on (publish-date ordering, prerelease
// exclusion).
//
// All of it runs offline against hand-built packuments — a real regression is
// not something you can conjure on the live registry to test against.

import { describe, expect, it } from "vitest";

import { checkAll, classifyPackage, report } from "../check-provenance-drift.mjs";

/** Build a packument. `versions` maps version → {attested, time}. */
function packument(latest, versions) {
  return {
    "dist-tags": { latest },
    time: Object.fromEntries(Object.entries(versions).map(([v, spec]) => [v, spec.time])),
    versions: Object.fromEntries(
      Object.entries(versions).map(([v, spec]) => [
        v,
        { dist: spec.attested ? { attestations: { url: "x" } } : {} },
      ]),
    ),
  };
}

describe("classifyPackage", () => {
  it("passes when the latest version carries an attestation", () => {
    const result = classifyPackage(
      "pkg",
      packument("2.0.0", {
        "1.0.0": { attested: true, time: "2026-01-01T00:00:00.000Z" },
        "2.0.0": { attested: true, time: "2026-02-01T00:00:00.000Z" },
      }),
    );
    expect(result).toEqual({ name: "pkg", status: "ok", latest: "2.0.0" });
  });

  it("flags a regression when an earlier version was attested and latest is not", () => {
    // This is the v2.12.0 shape exactly — the case the check exists for.
    const result = classifyPackage(
      "pkg",
      packument("2.12.0", {
        "2.11.0": { attested: true, time: "2026-08-01T00:00:00.000Z" },
        "2.12.0": { attested: false, time: "2026-08-26T00:00:00.000Z" },
      }),
    );
    expect(result).toEqual({
      name: "pkg",
      status: "regression",
      latest: "2.12.0",
      priorAttested: "2.11.0",
    });
  });

  it("only warns when no version was ever attested", () => {
    // zfb-darwin-x64 (#2625). No downgrade can fire with nothing to compare
    // against, so this must NOT be a failure or the weekly job is red forever.
    const result = classifyPackage(
      "pkg",
      packument("2.12.0", {
        "2.11.0": { attested: false, time: "2026-08-01T00:00:00.000Z" },
        "2.12.0": { attested: false, time: "2026-08-26T00:00:00.000Z" },
      }),
    );
    expect(result).toEqual({ name: "pkg", status: "never", latest: "2.12.0" });
  });

  it("names the most recent prior attested version, not merely any of them", () => {
    const result = classifyPackage(
      "pkg",
      packument("3.0.0", {
        "1.0.0": { attested: true, time: "2026-01-01T00:00:00.000Z" },
        "2.0.0": { attested: true, time: "2026-06-01T00:00:00.000Z" },
        "3.0.0": { attested: false, time: "2026-08-01T00:00:00.000Z" },
      }),
    );
    expect(result.priorAttested).toBe("2.0.0");
  });

  describe("pnpm parity", () => {
    it("orders by publish date, not semver", () => {
      // 3.0.0 is the current latest but was published BEFORE the 2.x backport,
      // and it is the backport that carries the attestation. By semver, 2.9.0
      // looks "earlier"; by publish date it is later, so pnpm would not treat
      // it as prior evidence — and neither may we.
      const result = classifyPackage(
        "pkg",
        packument("3.0.0", {
          "3.0.0": { attested: false, time: "2026-03-01T00:00:00.000Z" },
          "2.9.0": { attested: true, time: "2026-07-01T00:00:00.000Z" },
        }),
      );
      expect(result.status).toBe("never");
    });

    it("ignores prereleases when latest is stable", () => {
      // Matches pnpm >= 10.24. Without this an attested prerelease would raise
      // a downgrade that pnpm itself would never report.
      const result = classifyPackage(
        "pkg",
        packument("2.0.0", {
          "2.0.0-next.1": { attested: true, time: "2026-01-01T00:00:00.000Z" },
          "2.0.0": { attested: false, time: "2026-02-01T00:00:00.000Z" },
        }),
      );
      expect(result.status).toBe("never");
    });

    it("still considers prereleases when latest is itself a prerelease", () => {
      const result = classifyPackage(
        "pkg",
        packument("2.0.0-next.2", {
          "2.0.0-next.1": { attested: true, time: "2026-01-01T00:00:00.000Z" },
          "2.0.0-next.2": { attested: false, time: "2026-02-01T00:00:00.000Z" },
        }),
      );
      expect(result.status).toBe("regression");
      expect(result.priorAttested).toBe("2.0.0-next.1");
    });

    it("does not treat a LATER-published unattested version as prior evidence", () => {
      const result = classifyPackage(
        "pkg",
        packument("1.0.0", {
          "1.0.0": { attested: false, time: "2026-01-01T00:00:00.000Z" },
          "1.1.0": { attested: true, time: "2026-05-01T00:00:00.000Z" },
        }),
      );
      expect(result.status).toBe("never");
    });
  });

  describe("malformed registry data errors rather than passing", () => {
    it("errors when there is no latest dist-tag", () => {
      expect(classifyPackage("pkg", { versions: {} }).status).toBe("error");
    });

    it("errors when latest points at a version absent from the packument", () => {
      const result = classifyPackage("pkg", {
        "dist-tags": { latest: "9.9.9" },
        versions: {},
      });
      expect(result.status).toBe("error");
    });

    it("errors when the latest version has no publish time", () => {
      const result = classifyPackage("pkg", {
        "dist-tags": { latest: "1.0.0" },
        time: {},
        versions: { "1.0.0": { dist: {} } },
      });
      expect(result.status).toBe("error");
    });
  });
});

describe("checkAll", () => {
  it("turns a fetch failure into an error verdict instead of rejecting", () => {
    // One unreachable package must not take down the verdict for the other nine.
    return expect(
      checkAll({
        packages: ["good", "bad"],
        fetchOne: async (name) => {
          if (name === "bad") throw new Error("registry responded 500");
          return packument("1.0.0", {
            "1.0.0": { attested: true, time: "2026-01-01T00:00:00.000Z" },
          });
        },
      }),
    ).resolves.toEqual([
      { name: "good", status: "ok", latest: "1.0.0" },
      { name: "bad", status: "error", detail: "registry responded 500" },
    ]);
  });
});

describe("report", () => {
  const lines = () => {
    const out = [];
    return { out, log: (m) => out.push(m) };
  };

  it("exits 0 and emits no ::error when everything is attested", () => {
    const { out, log } = lines();
    expect(report([{ name: "a", status: "ok", latest: "1.0.0" }], { log })).toBe(0);
    expect(out.some((l) => l.startsWith("::error"))).toBe(false);
  });

  it("exits 0 for a never-attested package, but says so as a ::warning", () => {
    const { out, log } = lines();
    expect(report([{ name: "a", status: "never", latest: "1.0.0" }], { log })).toBe(0);
    expect(out.some((l) => l.startsWith("::warning"))).toBe(true);
    expect(out.some((l) => l.startsWith("::error"))).toBe(false);
  });

  it("exits 1 and emits a ::error naming both versions on a regression", () => {
    const { out, log } = lines();
    const code = report(
      [{ name: "a", status: "regression", latest: "2.12.0", priorAttested: "2.11.0" }],
      { log },
    );
    expect(code).toBe(1);
    const err = out.find((l) => l.startsWith("::error"));
    expect(err).toContain("2.12.0");
    expect(err).toContain("2.11.0");
    expect(err).toContain("ERR_PNPM_TRUST_DOWNGRADE");
  });

  it("exits 1 on an error verdict, so a registry outage is never a silent pass", () => {
    const { out, log } = lines();
    expect(report([{ name: "a", status: "error", detail: "boom" }], { log })).toBe(1);
    expect(out.some((l) => l.startsWith("::error"))).toBe(true);
  });
});
