import { describe, expect, it, vi } from "vitest";

import { checkForNewerVersion, compareSemver } from "../version-check.mjs";

describe("compareSemver", () => {
  it("orders major/minor/patch differences numerically, not lexically", () => {
    expect(compareSemver("1.0.0", "2.0.0")).toBe(-1);
    expect(compareSemver("2.0.0", "1.0.0")).toBe(1);
    expect(compareSemver("1.9.0", "1.10.0")).toBe(-1);
    expect(compareSemver("1.10.0", "1.9.0")).toBe(1);
    expect(compareSemver("1.2.3", "1.2.4")).toBe(-1);
    expect(compareSemver("1.2.4", "1.2.3")).toBe(1);
  });

  it("treats identical versions as equal", () => {
    expect(compareSemver("1.2.3", "1.2.3")).toBe(0);
    expect(compareSemver("0.1.0-next.75", "0.1.0-next.75")).toBe(0);
  });

  it("treats a missing trailing version part as 0 (1.0 == 1.0.0)", () => {
    expect(compareSemver("1.0", "1.0.0")).toBe(0);
    expect(compareSemver("1.0.0", "1.0")).toBe(0);
  });

  it("ranks a prerelease below the same release version", () => {
    expect(compareSemver("1.0.0-alpha", "1.0.0")).toBe(-1);
    expect(compareSemver("1.0.0", "1.0.0-alpha")).toBe(1);
  });

  it("orders the full semver.org prerelease precedence chain", () => {
    // https://semver.org/#spec-item-11 worked example.
    const chain = [
      "1.0.0-alpha",
      "1.0.0-alpha.1",
      "1.0.0-alpha.beta",
      "1.0.0-beta",
      "1.0.0-beta.2",
      "1.0.0-beta.11",
      "1.0.0-rc.1",
      "1.0.0",
    ];
    for (let i = 0; i < chain.length - 1; i++) {
      expect(compareSemver(chain[i], chain[i + 1])).toBe(-1);
      expect(compareSemver(chain[i + 1], chain[i])).toBe(1);
    }
  });

  it("compares numeric prerelease identifiers numerically (beta.2 < beta.11)", () => {
    // Plain string comparison would rank "11" before "2".
    expect(compareSemver("1.0.0-beta.2", "1.0.0-beta.11")).toBe(-1);
    expect(compareSemver("1.0.0-beta.11", "1.0.0-beta.2")).toBe(1);
  });

  it("ranks numeric prerelease identifiers below alphanumeric ones", () => {
    expect(compareSemver("1.0.0-1", "1.0.0-alpha")).toBe(-1);
    expect(compareSemver("1.0.0-alpha", "1.0.0-1")).toBe(1);
  });

  it("ranks a shorter prerelease identifier set below a longer one sharing the same prefix", () => {
    expect(compareSemver("1.0.0-alpha", "1.0.0-alpha.1")).toBe(-1);
    expect(compareSemver("1.0.0-alpha.1", "1.0.0-alpha")).toBe(1);
  });

  it("real-world case: create-zfb's own next-prerelease channel", () => {
    expect(compareSemver("0.1.0-next.6", "0.1.0-next.8")).toBe(-1);
    expect(compareSemver("0.1.0-next.8", "0.1.0-next.75")).toBe(-1);
    expect(compareSemver("0.1.0-next.75", "0.1.0")).toBe(-1);
  });

  describe("build metadata", () => {
    // Per semver spec (https://semver.org/#spec-item-10), build metadata
    // (the `+...` suffix) MUST be ignored when determining precedence. The
    // implementation drops it before comparing (`v.split("+")[0]`) — these
    // tests confirm that behavior matches the spec; no divergence found.
    it("ignores build metadata when versions are otherwise identical", () => {
      expect(compareSemver("1.0.0+build1", "1.0.0+build2")).toBe(0);
      expect(compareSemver("1.0.0", "1.0.0+build1")).toBe(0);
      expect(compareSemver("1.0.0+build1", "1.0.0")).toBe(0);
    });

    it("ignores build metadata attached after a prerelease tag", () => {
      expect(compareSemver("1.0.0-beta+exp.sha.5114f85", "1.0.0-beta")).toBe(0);
      expect(compareSemver("1.0.0-alpha+001", "1.0.0-alpha+002")).toBe(0);
    });

    it("still orders correctly by the non-build parts when build metadata is present on both sides", () => {
      expect(compareSemver("1.0.0+build1", "1.0.1+build1")).toBe(-1);
      expect(compareSemver("1.0.0-alpha+b1", "1.0.0-beta+b1")).toBe(-1);
    });
  });
});

describe("checkForNewerVersion", () => {
  function okResponse(body) {
    return { ok: true, json: async () => body };
  }

  function hangingFetch() {
    // Simulates a registry that never responds — resolves/rejects only
    // when the AbortController tied to `signal` fires, mirroring how the
    // real global fetch reacts to abort.
    return vi.fn((_url, { signal }) => {
      return new Promise((_resolve, reject) => {
        signal.addEventListener("abort", () => {
          const err = new Error("The operation was aborted");
          err.name = "AbortError";
          reject(err);
        });
      });
    });
  }

  it("returns a warning when the registry reports a newer version", async () => {
    const fetchFn = vi.fn(async () => okResponse({ version: "0.2.0" }));
    const message = await checkForNewerVersion({ version: "0.1.0", fetchFn });
    expect(message).not.toBeNull();
    expect(message).toContain("create-zfb@0.1.0");
    expect(message).toContain("@latest is 0.2.0");
    expect(message).toContain("npm create zfb@latest");
  });

  it("returns null when the installed version is already current", async () => {
    const fetchFn = vi.fn(async () => okResponse({ version: "0.1.0" }));
    const message = await checkForNewerVersion({ version: "0.1.0", fetchFn });
    expect(message).toBeNull();
  });

  it("returns null when the installed version is ahead of @latest (e.g. a prerelease channel)", async () => {
    const fetchFn = vi.fn(async () => okResponse({ version: "0.1.0" }));
    const message = await checkForNewerVersion({ version: "0.2.0-next.1", fetchFn });
    expect(message).toBeNull();
  });

  it("requests the registry's `latest` endpoint with a version-stamped User-Agent", async () => {
    const fetchFn = vi.fn(async () => okResponse({ version: "0.1.0" }));
    await checkForNewerVersion({ version: "0.1.0", fetchFn });
    expect(fetchFn).toHaveBeenCalledWith(
      "https://registry.npmjs.org/create-zfb/latest",
      expect.objectContaining({
        headers: expect.objectContaining({ "User-Agent": "create-zfb/0.1.0" }),
      }),
    );
  });

  it("fails silently (returns null) on a non-ok registry response", async () => {
    const fetchFn = vi.fn(async () => ({ ok: false, json: async () => ({}) }));
    await expect(checkForNewerVersion({ version: "0.1.0", fetchFn })).resolves.toBeNull();
  });

  it("fails silently on a network error (fetch rejects)", async () => {
    const fetchFn = vi.fn(async () => {
      throw new Error("ECONNREFUSED");
    });
    await expect(checkForNewerVersion({ version: "0.1.0", fetchFn })).resolves.toBeNull();
  });

  it("fails silently on malformed JSON", async () => {
    const fetchFn = vi.fn(async () => ({
      ok: true,
      json: async () => {
        throw new SyntaxError("Unexpected token");
      },
    }));
    await expect(checkForNewerVersion({ version: "0.1.0", fetchFn })).resolves.toBeNull();
  });

  it("fails silently when the response body has no version field", async () => {
    const fetchFn = vi.fn(async () => okResponse({}));
    await expect(checkForNewerVersion({ version: "0.1.0", fetchFn })).resolves.toBeNull();
  });

  it("suppresses the warning when the registry doesn't respond within a configured timeout", async () => {
    const fetchFn = hangingFetch();
    const message = await checkForNewerVersion({ version: "0.1.0", fetchFn, timeoutMs: 20 });
    expect(message).toBeNull();
  });

  it("honors the default 1.5s timeout and never throws", async () => {
    vi.useFakeTimers();
    try {
      const fetchFn = hangingFetch();
      const promise = checkForNewerVersion({ version: "0.1.0", fetchFn });
      await vi.advanceTimersByTimeAsync(1500);
      await expect(promise).resolves.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});
