import assert from "node:assert/strict";
import { test } from "vitest";
import { directChildPids, selectRunningBrowser } from "../lib/browser.mjs";

const ROOT = "/home/u/.cache/ms-playwright";
const SHELL =
  ROOT + "/chromium_headless_shell-1228/chrome-headless-shell-linux64/chrome-headless-shell";
const FULL = ROOT + "/chromium-1228/chrome-linux64/chrome";
const links = (map) => (p) => {
  const pid = Number(/\/proc\/(\d+)\/exe/.exec(p)[1]);
  if (!(pid in map)) throw Object.assign(Error("gone"), { code: "ENOENT" });
  return map[pid];
};

test("the new headless-shell child is the running browser, not the library default", () => {
  const got = selectRunningBrowser({
    before: [10, 11],
    after: [10, 11, 12, 13],
    browsersRoot: ROOT,
    readLink: links({ 10: FULL, 11: "/usr/bin/perl", 12: "/usr/bin/python3", 13: SHELL }),
  });
  assert.deepEqual(got, { pid: 13, path: SHELL });
});

test("refuses zero, two, foreign-root or vanished candidates", () => {
  const cases = [
    { after: [10], map: { 10: SHELL } },
    { after: [10, 12, 13], map: { 10: SHELL, 12: SHELL, 13: FULL } },
    { after: [10, 12], map: { 10: SHELL, 12: "/opt/other/chrome" } },
    { after: [10, 12], map: { 10: SHELL } },
  ];
  for (const c of cases)
    assert.throws(
      () =>
        selectRunningBrowser({
          before: [10],
          after: c.after,
          browsersRoot: ROOT,
          readLink: links(c.map),
        }),
      /expected one launched browser child/,
    );
});

test("direct children are read from every task's children file", () => {
  const files = { "/proc/5/task/5/children": "7 9 ", "/proc/5/task/6/children": "8\n" };
  const got = directChildPids(
    5,
    (p) => {
      if (!(p in files)) throw Object.assign(Error("gone"), { code: "ENOENT" });
      return files[p];
    },
    () => ["5", "6", "99"],
  );
  assert.deepEqual(got, [7, 8, 9]);
});
