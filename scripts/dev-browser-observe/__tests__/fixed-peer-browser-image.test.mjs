import assert from "node:assert/strict";
import { test } from "vitest";
import { fixedPeerBrowserImageErrors } from "../lib/browser.mjs";

// #4870 F4: the fixed Next pin launches chromium_headless_shell; the library
// default `chrome` is pinned separately and must never be the running image.
const SHELL =
  "/pw/chromium_headless_shell-1228/chrome-headless-shell-linux64/chrome-headless-shell";
const CHROME = "/pw/chromium-1228/chrome-linux64/chrome";
const pin = {
  executablePath: SHELL,
  executableSha256: "1".repeat(64),
  defaultExecutableSha256: "2".repeat(64),
};
const realpath = (p) => {
  if (p !== SHELL) throw Object.assign(Error("missing"), { code: "ENOENT" });
  return p;
};
const good = {
  executablePath: CHROME,
  executableSha256: "2".repeat(64),
  launchedExecutablePath: SHELL,
  launchedExecutableSha256: "1".repeat(64),
  runningExecutablePath: SHELL,
  runningExecutableSha256: "1".repeat(64),
};

test("the pinned headless shell running by /proc/<pid>/exe is admitted", () => {
  assert.deepEqual(fixedPeerBrowserImageErrors(good, pin, realpath), []);
});

test.each([
  [
    "running full chrome",
    { runningExecutablePath: CHROME, runningExecutableSha256: "2".repeat(64) },
    /running browser digest mismatch; running browser path mismatch/,
  ],
  [
    "running digest differs",
    { runningExecutableSha256: "3".repeat(64) },
    /running browser digest mismatch/,
  ],
  [
    "running path differs",
    { runningExecutablePath: SHELL + ".old" },
    /running browser path mismatch/,
  ],
  [
    "running receipt unreadable",
    {
      runningExecutablePath: null,
      runningExecutableSha256: null,
      runningExecutableError: "expected one launched browser child, found []",
    },
    /running browser image unreadable/,
  ],
  [
    "running receipt absent",
    { runningExecutablePath: undefined, runningExecutableSha256: undefined },
    /running browser digest mismatch/,
  ],
  [
    "launched digest differs",
    { launchedExecutableSha256: "4".repeat(64) },
    /launched browser digest mismatch/,
  ],
  ["launched default chrome", { launchedExecutablePath: CHROME }, /launched browser path mismatch/],
  [
    "library default changed",
    { executableSha256: "5".repeat(64) },
    /library-default browser digest mismatch/,
  ],
])("refuses: %s", (_name, change, pattern) => {
  const errors = fixedPeerBrowserImageErrors({ ...good, ...change }, pin, realpath);
  assert(errors.length > 0);
  assert.match(errors.join("; "), pattern);
});

test("an unresolvable pinned path refuses", () => {
  const errors = fixedPeerBrowserImageErrors(
    good,
    { ...pin, executablePath: "/nowhere" },
    realpath,
  );
  assert.match(errors.join("; "), /pinned browser path unresolved: ENOENT/);
});
