// Tests for scripts/verify-npm-provenance-expectations.mjs over an injected fake
// `npm view`. Output shapes mirror the live registry: an attested version prints
// a JSON object with provenance.predicateType; an unattested one prints nothing
// and exits 0; a missing version fails with E404.

import { describe, expect, it } from "vite-plus/test";

import {
  SLSA_PROVENANCE_V1,
  classifyNpmError,
  parseExpectations,
  verifyExpectations,
} from "../verify-npm-provenance-expectations.mjs";

const ATTESTED = JSON.stringify({
  url: "https://registry.npmjs.org/-/npm/v1/attestations/@takazudo%2fzfb@9.9.9",
  provenance: { predicateType: SLSA_PROVENANCE_V1 },
});

function npmError(message) {
  const error = new Error(`Command failed: npm view\n${message}`);
  error.stdout = "";
  return error;
}

// responses: name@version → array of results consumed per call (last repeats).
function fakeNpm(responses) {
  const calls = [];
  return {
    calls,
    npm: async (args) => {
      calls.push(args);
      const queue = responses[args[1]];
      const next = queue.length > 1 ? queue.shift() : queue[0];
      if (next instanceof Error) throw next;
      return next;
    },
  };
}

const opts = (npm, extra = {}) => ({
  npm,
  wait: async () => {},
  attempts: 3,
  delayMs: 0,
  log: () => {},
  ...extra,
});

describe("verifyExpectations", () => {
  it("passes when every package matches (attested and explicitly unattested)", async () => {
    const { npm } = fakeNpm({
      "@takazudo/zfb@9.9.9": [ATTESTED],
      "@takazudo/zfb-darwin-x64@9.9.9": [""],
    });
    const { failed } = await verifyExpectations(
      [
        {
          name: "@takazudo/zfb",
          version: "9.9.9",
          expected: "attested",
          outcome: "publish-attempted",
        },
        {
          name: "@takazudo/zfb-darwin-x64",
          version: "9.9.9",
          expected: "none",
          outcome: "publish-attempted",
        },
      ],
      opts(npm),
    );
    expect(failed).toBe(0);
  });

  it("fails when an expected attestation is missing", async () => {
    const { npm, calls } = fakeNpm({ "@takazudo/zfb@9.9.9": [""] });
    const lines = [];
    const { results, failed } = await verifyExpectations(
      [
        {
          name: "@takazudo/zfb",
          version: "9.9.9",
          expected: "attested",
          outcome: "already-present",
        },
      ],
      opts(npm, { log: (l) => lines.push(l) }),
    );
    expect(failed).toBe(1);
    expect(results[0].verdict).toBe("mismatch");
    expect(calls).toHaveLength(3); // retried within the budget before failing
    expect(lines[0]).toMatch(/^::error title=npm provenance expectation \(mismatch\)::/);
  });

  it("fails at once when an attestation exists but none was expected", async () => {
    const { npm, calls } = fakeNpm({ "@takazudo/zfb-darwin-x64@9.9.9": [ATTESTED] });
    const { results } = await verifyExpectations(
      [
        {
          name: "@takazudo/zfb-darwin-x64",
          version: "9.9.9",
          expected: "none",
          outcome: "publish-attempted",
        },
      ],
      opts(npm),
    );
    expect(results[0]).toMatchObject({ ok: false, verdict: "mismatch" });
    expect(calls).toHaveLength(1);
  });

  it("fails on an attestation of an unexpected predicate type", async () => {
    const other = JSON.stringify({
      provenance: { predicateType: "https://slsa.dev/provenance/v0.2" },
    });
    const { npm } = fakeNpm({ "@takazudo/zfb@9.9.9": [other] });
    const { results } = await verifyExpectations(
      [{ name: "@takazudo/zfb", version: "9.9.9", expected: "attested", outcome: "x" }],
      opts(npm),
    );
    expect(results[0]).toMatchObject({ ok: false, verdict: "mismatch" });
  });

  it("retries read-after-write lag (E404) and then verifies", async () => {
    const { npm } = fakeNpm({
      "@takazudo/zfb@9.9.9": [
        npmError("npm error code E404\nnpm error 404 No match found for version 9.9.9"),
        ATTESTED,
      ],
    });
    const { failed } = await verifyExpectations(
      [{ name: "@takazudo/zfb", version: "9.9.9", expected: "attested", outcome: "x" }],
      opts(npm),
    );
    expect(failed).toBe(0);
  });

  it("reports a version that never becomes visible distinctly from a mismatch", async () => {
    const { npm } = fakeNpm({ "@takazudo/zfb@9.9.9": [npmError("npm error code E404")] });
    const { results } = await verifyExpectations(
      [{ name: "@takazudo/zfb", version: "9.9.9", expected: "none", outcome: "x" }],
      opts(npm),
    );
    expect(results[0].verdict).toBe("not-visible");
  });

  it("reports a persistent network error as registry-error, never as absent", async () => {
    const { npm } = fakeNpm({ "@takazudo/zfb@9.9.9": [npmError("npm error code ECONNRESET")] });
    const { results } = await verifyExpectations(
      [{ name: "@takazudo/zfb", version: "9.9.9", expected: "none", outcome: "x" }],
      opts(npm),
    );
    expect(results[0].verdict).toBe("registry-error");
  });

  it("does not retry an auth error", async () => {
    const { npm, calls } = fakeNpm({ "@takazudo/zfb@9.9.9": [npmError("npm error code E401")] });
    const { results } = await verifyExpectations(
      [{ name: "@takazudo/zfb", version: "9.9.9", expected: "attested", outcome: "x" }],
      opts(npm),
    );
    expect(results[0].verdict).toBe("auth-error");
    expect(calls).toHaveLength(1);
  });
});

describe("parseExpectations / classifyNpmError", () => {
  it("parses the publish helper's TSV and rejects malformed lines", () => {
    expect(
      parseExpectations(
        "a\t1.0.0\tattested\tpublish-attempted\n\nb\t1.0.0\tnone\talready-present\n",
      ),
    ).toEqual([
      { name: "a", version: "1.0.0", expected: "attested", outcome: "publish-attempted" },
      { name: "b", version: "1.0.0", expected: "none", outcome: "already-present" },
    ]);
    expect(() => parseExpectations("a\t1.0.0\tmaybe\n")).toThrow(/malformed/);
  });

  it("classifies npm errors", () => {
    expect(classifyNpmError("npm error code E404")).toBe("not-visible");
    expect(classifyNpmError("npm error code ETARGET")).toBe("not-visible");
    expect(classifyNpmError("npm error code E403")).toBe("auth");
    expect(classifyNpmError("npm error code ETIMEDOUT")).toBe("transient");
  });
});
