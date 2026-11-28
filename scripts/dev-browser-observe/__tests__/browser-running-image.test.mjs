import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readlinkSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "vitest";
import {
  directChildPids,
  fixedPeerBrowserImageErrors,
  runningBrowserReceipt,
  selectRunningBrowser,
} from "../lib/browser.mjs";

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

// Review 1 finding 5: the running-image receipt has never been read on a real v4
// launch. This is the strongest coverage short of one: real processes, the real
// /proc/<pid>/children and /proc/<pid>/exe reads and the real receipt function
// launchOwnedBrowser calls, against a ms-playwright-shaped tree whose "browsers"
// are copies of a small system binary. It is not a browser launch and does not
// prove the real Playwright child layout.
const STAND_IN = "/bin/sleep";
const digestOf = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");

async function launchedTree(run) {
  const dir = mkdtempSync(join(tmpdir(), "running-image-"));
  const root = join(dir, "ms-playwright");
  const chrome = join(root, "chromium-1228/chrome-linux64/chrome");
  const shell = join(
    root,
    "chromium_headless_shell-1228/chrome-headless-shell-linux64/chrome-headless-shell",
  );
  const outside = join(dir, "elsewhere/chrome-headless-shell");
  for (const path of [chrome, shell, outside]) {
    mkdirSync(dirname(path), { recursive: true });
    copyFileSync(STAND_IN, path);
    chmodSync(path, 0o755);
  }
  // Trailing bytes keep the ELF runnable and make the library default's digest
  // differ from the shell's, as the two real images do.
  appendFileSync(chrome, "library-default");
  assert.notEqual(digestOf(chrome), digestOf(shell));
  const children = [];
  const start = async (path) => {
    const child = spawn(path, ["30"], { stdio: "ignore" });
    children.push(child);
    // Wait until the kernel reports the exec'd image, as after a browser launch.
    for (let i = 0; i < 200; i++) {
      try {
        if (readlinkSync(`/proc/${child.pid}/exe`) === path) return child;
      } catch {}
      await new Promise((r) => setTimeout(r, 5));
    }
    throw new Error("stand-in did not exec: " + path);
  };
  try {
    return await run({ chrome, shell, outside, start });
  } finally {
    for (const child of children) child.kill("SIGKILL");
    await Promise.all(
      children.map(
        (c) => (c.exitCode ?? c.signalCode) !== null || new Promise((r) => c.once("exit", r)),
      ),
    );
    rmSync(dir, { recursive: true, force: true });
  }
}

const receipt = (chrome, before) =>
  runningBrowserReceipt({
    executablePath: chrome,
    before,
    digest: async (path) => digestOf(path),
  });

test.skipIf(process.platform !== "linux")(
  "a real launched child's /proc receipt admits the pinned shell and nothing else",
  async () => {
    await launchedTree(async ({ chrome, shell, outside, start }) => {
      const pin = {
        executablePath: shell,
        executableSha256: digestOf(shell),
        defaultExecutableSha256: digestOf(chrome),
      };
      const identity = (running) => ({
        executablePath: chrome,
        executableSha256: digestOf(chrome),
        launchedExecutablePath: shell,
        launchedExecutableSha256: digestOf(shell),
        ...running,
      });
      // Admitted: the one new direct child executes the pinned shell.
      let before = directChildPids();
      const child = await start(shell);
      const good = await receipt(chrome, before);
      assert.deepEqual(good, {
        runningExecutablePath: shell,
        runningExecutableSha256: digestOf(`/proc/${child.pid}/exe`),
      });
      assert.equal(good.runningExecutableSha256, pin.executableSha256);
      assert.deepEqual(fixedPeerBrowserImageErrors(identity(good), pin), []);
      // Refused: the library-default chrome is what actually runs.
      before = directChildPids();
      await start(chrome);
      const full = await receipt(chrome, before);
      assert.equal(full.runningExecutablePath, chrome);
      assert.match(
        fixedPeerBrowserImageErrors(identity(full), pin).join("; "),
        /running browser digest mismatch; running browser path mismatch/,
      );
      // Refused: a same-named shell outside the ms-playwright root is never selected.
      before = directChildPids();
      await start(outside);
      const foreign = await receipt(chrome, before);
      assert.match(
        foreign.runningExecutableError,
        /expected one launched browser child, found \[\]/,
      );
      assert.match(
        fixedPeerBrowserImageErrors(identity(foreign), pin).join("; "),
        /running browser image unreadable/,
      );
      // Refused: two new browser children are ambiguous.
      before = directChildPids();
      await start(shell);
      await start(shell);
      const two = await receipt(chrome, before);
      assert.match(two.runningExecutableError, /expected one launched browser child/);
      assert.equal(two.runningExecutableSha256, null);
    });
  },
);

// Review 2 correction D (mutant W15): the digest is taken of the image the
// process executes, /proc/<pid>/exe, never of the path that readlink returned,
// which could name different bytes after a swap or an overmount.
test.skipIf(process.platform !== "linux")(
  "the running digest is read through /proc/<pid>/exe",
  async () => {
    await launchedTree(async ({ chrome, shell, start }) => {
      const before = directChildPids();
      const child = await start(shell);
      const calls = [];
      const got = await runningBrowserReceipt({
        executablePath: chrome,
        before,
        digest: async (path, role) => (calls.push([path, role]), "d".repeat(64)),
      });
      assert.deepEqual(calls, [[`/proc/${child.pid}/exe`, "running-launched"]]);
      assert.deepEqual(got, {
        runningExecutablePath: shell,
        runningExecutableSha256: "d".repeat(64),
      });
    });
  },
);
