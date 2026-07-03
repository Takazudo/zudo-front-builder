// Extracted from create-zfb.mjs so compareSemver and checkForNewerVersion can
// be unit-tested directly. create-zfb.mjs has side-effecting top-level code
// (fetch, spawnSync), so importing it in a test would run the whole CLI —
// this module has none, and is safe to import.

// Returns -1 if a < b, 0 if a === b, 1 if a > b.
// Handles prerelease suffixes: 0.1.0-next.6 < 0.1.0-next.8 < 0.1.0
export function compareSemver(a, b) {
  const splitRelease = (v) => {
    // Drop SemVer build metadata (`+...`) before splitting — it is ignored
    // for precedence and would otherwise yield NaN numeric parts.
    const [main, pre] = v.split("+")[0].split(/-(.+)/, 2);
    return { parts: main.split(".").map(Number), pre: pre ?? null };
  };
  const ra = splitRelease(a);
  const rb = splitRelease(b);
  for (let i = 0; i < Math.max(ra.parts.length, rb.parts.length); i++) {
    const pa = ra.parts[i] ?? 0;
    const pb = rb.parts[i] ?? 0;
    if (pa !== pb) return pa < pb ? -1 : 1;
  }
  // same numeric parts — prerelease is less than no prerelease (semver rule)
  if (ra.pre !== null && rb.pre === null) return -1;
  if (ra.pre === null && rb.pre !== null) return 1;
  if (ra.pre !== null && rb.pre !== null) {
    // Compare dot-separated prerelease identifiers per semver: numeric
    // identifiers compare numerically (so next.9 < next.10), numeric ranks
    // below alphanumeric, and a shorter prefix ranks lower. Plain string
    // `<`/`>` would order next.10 before next.9 lexicographically.
    const sa = ra.pre.split(".");
    const sb = rb.pre.split(".");
    for (let i = 0; i < Math.max(sa.length, sb.length); i++) {
      if (sa[i] === undefined) return -1;
      if (sb[i] === undefined) return 1;
      const na = /^\d+$/.test(sa[i]);
      const nb = /^\d+$/.test(sb[i]);
      if (na && nb) {
        const da = Number(sa[i]);
        const db = Number(sb[i]);
        if (da !== db) return da < db ? -1 : 1;
      } else if (na !== nb) {
        return na ? -1 : 1;
      } else if (sa[i] !== sb[i]) {
        return sa[i] < sb[i] ? -1 : 1;
      }
    }
  }
  return 0;
}

/**
 * Checks the npm registry for a newer create-zfb release than `version` and
 * returns a warning message if the installed version is stale, or null if
 * up to date. Never throws — network failure, timeout (AbortError), and
 * parse errors are all treated as "no warning" so a registry hiccup can
 * never block scaffolding. `fetchFn` and `timeoutMs` are injectable so tests
 * don't need to hit the real network or wait out the real timeout.
 */
export async function checkForNewerVersion({ version, fetchFn = fetch, timeoutMs = 1500 } = {}) {
  try {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      const res = await fetchFn("https://registry.npmjs.org/create-zfb/latest", {
        signal: controller.signal,
        headers: {
          "User-Agent": `create-zfb/${version}`,
          Accept: "application/json",
        },
      });
      if (!res.ok) return null;
      const data = await res.json();
      const latest = data?.version;
      if (typeof latest !== "string" || typeof version !== "string") return null;
      if (compareSemver(version, latest) >= 0) return null;
      return (
        `[create-zfb] You are running create-zfb@${version} but @latest is ${latest}.\n` +
        `[create-zfb] pnpm 11's minimumReleaseAge may have downgraded the install.\n` +
        `[create-zfb] Re-run with: pnpm create zfb@latest --config.minimumReleaseAge=0\n` +
        `[create-zfb] Or use: npm create zfb@latest\n`
      );
    } finally {
      // Keep the abort signal armed across both the header and body reads.
      // Only cancel the timer once the body has been fully consumed (or on
      // any error path), so a proxy that stalls after headers cannot hang
      // the scaffolder indefinitely.
      clearTimeout(timer);
    }
  } catch {
    return null;
  }
}
