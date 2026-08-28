// Tests for scripts/check-provenance-drift.mjs.
//
// The whole value of this check is the DISCRIMINATION it makes: a package that
// LOST an attestation must fail, and one that never had it must fail too.
// Collapse those cases and the check is worthless: it either misses the
// v2.12.0 regression (#2623) or silently accepts a newly added package that
// never gets attested. So the bulk of these tests pin that policy boundary and
// the two pnpm-matching rules that decide which side of it a package falls on
// (publish-date ordering, prerelease exclusion).
//
// All of it runs offline against hand-built packuments — a real regression is
// not something you can conjure on the live registry to test against.

import { describe, expect, it } from "vitest";

import { checkAll, classifyPackage, fetchPackument, report } from "../check-provenance-drift.mjs";

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

  it("flags an unattested → attested → unattested history as a regression", () => {
    // The attested middle release is prior evidence, so the latest release is
    // a trust downgrade.
    const result = classifyPackage(
      "pkg",
      packument("2.12.0", {
        "2.10.0": { attested: false, time: "2026-07-01T00:00:00.000Z" },
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

  it("fails every never-attested package under the all-published policy", () => {
    const result = classifyPackage(
      "@takazudo/zfb-darwin-x64",
      packument("2.13.0", {
        "2.12.0": { attested: false, time: "2026-08-26T00:00:00.000Z" },
        "2.13.0": { attested: false, time: "2026-08-27T00:00:00.000Z" },
      }),
    );
    expect(result).toEqual({
      name: "@takazudo/zfb-darwin-x64",
      status: "never",
      latest: "2.13.0",
    });

    const out = [];
    const log = (message) => out.push(message);
    expect(report([result], { log })).toBe(1);
    expect(out.some((l) => l.startsWith("::error"))).toBe(true);
    expect(out.some((l) => l.startsWith("::warning"))).toBe(false);
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

describe("fetchPackument retry", () => {
  // The retry exists so one dropped packet on a weekly scheduled leg doesn't
  // file a tracking issue and page IFTTT. That makes "which failures retry"
  // load-bearing: retry a 404 and a genuinely-gone package wastes the budget
  // and still fails; don't retry a 503 and the blip becomes a false alarm.
  const ok = { ok: true, json: async () => ({ marker: "packument" }) };
  const status = (code) => ({ ok: false, status: code, statusText: "x" });

  /** Records sleeps instead of taking them, so the tests stay instant. */
  const harness = (responses) => {
    const calls = [];
    const sleeps = [];
    const fetchImpl = async () => {
      const next = responses[calls.length];
      calls.push(next);
      if (next instanceof Error) throw next;
      return next;
    };
    return { calls, sleeps, fetchImpl, sleepImpl: async (n) => void sleeps.push(n) };
  };

  it("returns the packument without sleeping when the first attempt succeeds", async () => {
    const h = harness([ok]);
    await expect(fetchPackument("pkg", h)).resolves.toEqual({ marker: "packument" });
    expect(h.calls).toHaveLength(1);
    expect(h.sleeps).toEqual([]);
  });

  it("retries a network error and succeeds on a later attempt", async () => {
    const h = harness([new Error("ECONNRESET"), ok]);
    await expect(fetchPackument("pkg", h)).resolves.toEqual({ marker: "packument" });
    expect(h.calls).toHaveLength(2);
    expect(h.sleeps).toEqual([1]);
  });

  it("retries a 5xx and a 429", async () => {
    const h5 = harness([status(503), ok]);
    await expect(fetchPackument("pkg", h5)).resolves.toBeTruthy();
    expect(h5.calls).toHaveLength(2);

    const h429 = harness([status(429), ok]);
    await expect(fetchPackument("pkg", h429)).resolves.toBeTruthy();
    expect(h429.calls).toHaveLength(2);
  });

  it("does NOT retry a 404 — that is a real answer, not a blip", async () => {
    const h = harness([status(404), ok]);
    await expect(fetchPackument("pkg", h)).rejects.toThrow(/404/);
    expect(h.calls).toHaveLength(1);
    expect(h.sleeps).toEqual([]);
  });

  it("gives up after the attempt budget and reports the last failure", async () => {
    const h = harness([status(500), status(500), status(500)]);
    await expect(fetchPackument("pkg", h)).rejects.toThrow(/registry responded 500/);
    expect(h.calls).toHaveLength(3);
    // Backs off between attempts, but never sleeps after the final one.
    expect(h.sleeps).toEqual([1, 2]);
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

  it("exits 1 and emits a ::error for a never-attested package", () => {
    const { out, log } = lines();
    expect(report([{ name: "a", status: "never", latest: "1.0.0" }], { log })).toBe(1);
    expect(out.some((l) => l.startsWith("::error"))).toBe(true);
    expect(out.some((l) => l.startsWith("::warning"))).toBe(false);
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
