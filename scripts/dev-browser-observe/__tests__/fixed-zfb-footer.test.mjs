import { zfbFooterStopPopulation } from "../lib/fixed-family-admission.mjs";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import { test } from "vitest";
import { ObserverPage, nativePageReload } from "../lib/browser.mjs";
import { semanticErrors, profileSchemaFor } from "../lib/profile.mjs";
import { validateAs } from "../lib/schema.mjs";

test("ZFB footer accepts one local upgrade and authenticated native page payload", async () => {
  const session = new EventEmitter();
  session.send = async (op) => assert.equal(op, "Network.enable");
  const page = new ObserverPage({ newCDPSession: async () => session }, "", () => 10n);
  page.page = { frames: () => [{}] };
  page.baseUrl = "http://127.0.0.1:5555";
  await page.trackNativeReload();
  const valid = "ws://127.0.0.1:5555/__zfb/reload/ws";
  for (const [id, url, status] of [
    ["foreign", "ws://elsewhere:5555/__zfb/reload/ws", 101],
    ["path", valid + "?other", 101],
    ["not-upgraded", valid, 200],
    ["ok", valid, 101],
  ]) {
    session.emit("Network.webSocketCreated", { requestId: id, url });
    session.emit("Network.webSocketHandshakeResponseReceived", {
      requestId: id,
      response: { status },
    });
    session.emit("Network.webSocketFrameReceived", {
      requestId: id,
      response: { opcode: 1, payloadData: JSON.stringify({ event: "page", data: "{}" }) },
    });
  }
  assert.deepEqual([...page.peerSocketTracking.keys()], ["ok"]);
  assert.equal(page.wsFrames.length, 1);
  const frame = page.wsFrames[0];
  const evidence = {
    wsFrames: [frame],
    navigations: [{ ns: "12", url: page.baseUrl + "/" }],
    docInits: [{ ns: "13", designatedMain: true, docUUID: "B", url: page.baseUrl + "/" }],
  };
  const check = () => nativePageReload(evidence, "ok", page.baseUrl + "/", "A", "B", 9n, 20n);
  assert(check());
  frame.ns = "14";
  assert(!check());
  frame.ns = "10";
  frame.pageSignal = false;
  assert(!check());
  session.emit("Network.webSocketClosed", { requestId: "ok" });
  assert.equal(page.peerSocketTracking.size, 0);
});
test.each(["original-shared-layout.v1", "original-shared-layout-join-owned.v2"])(
  "native shared footer safety is a separate exact opt-in: %s",
  (variant) => {
    const p = JSON.parse(
      readFileSync(
        new URL(
          "../profiles/field-notes-fixed-zfb-original-memoized-100.example.json",
          import.meta.url,
        ),
      ),
    );
    p.fixedRssSafety = "linux-fixed-zfb.v4";
    p.fixedZfbFooter = variant;
    p.fixedPeerBrowser = {
      executablePath: "/chrome",
      executableSha256: "a".repeat(64),
      defaultExecutableSha256: "b".repeat(64),
    };
    const footer = p.scenarios.find((x) => x.id === "fn-shared-component");
    delete footer.ops[0].repeat;
    p.scenarios = [footer];
    p.server.readyTimeoutMs = 300000;
    p.limits.wallCeilingMs = 7200000;
    Object.assign(p.timeouts, { attemptMs: 60000, supplementalMs: 60000, restoreMs: 120000 });
    const errors = (q) => [...validateAs(profileSchemaFor(q), q), ...semanticErrors(q)];
    assert.deepEqual(errors(p), []);
    for (const change of [
      (q) => (q.fixedRssSafety = "linux-fixed-zfb.v3"),
      (q) => (q.framework = "astro"),
      (q) => (q.fixedFamilyAdmission = "blank-before-custody.v1"),
      (q) => (q.fixedFamilyAdmission = "blank-join-owned.v2"),
      (q) => (q.fixture.adapter = "recovered-snapshot"),
      (q) => (q.fixture.importedCss = false),
      (q) => delete q.fixedPeerBrowser,
      (q) => (q.scenarios[0].ops[0].op = "edit-managed-css"),
      (q) => (q.scenarios[0].ops[0].repeat = { warmups: 0, measured: 1 }),
    ]) {
      const wrong = structuredClone(p);
      change(wrong);
      assert(errors(wrong).length);
    }
  },
);

test("native ZFB final population preserves physical cleanup while rejecting an unexpected birth", () => {
  const b = (pid, ppid, pgid) => ({
    pid,
    ppid,
    pgid,
    startTicks: String(pid),
    bootId: "boot",
    birthVersion: "linux-proc-v1",
  });
  const root = b(30, 29, 30),
    dev = b(31, 30, 30),
    startupBirth = b(32, 31, 32);
  const cap = { root, dev, startupBirth };
  const exit = { exitCode: 0 };
  const stop = {
    absent: true,
    rootAlive: true,
    remaining: [],
    ownedBefore: [root, dev],
    known: [root, dev, startupBirth],
    exit,
    steps: [{ signal: "SIGINT", delivery: "sent", target: "group:30" }],
    endings: [
      { pid: 30, endedAfter: "SIGINT", exit },
      { pid: 31, endedAfter: "SIGINT" },
      { pid: 32, endedAfter: "before-stop" },
    ],
  };
  assert(zfbFooterStopPopulation(cap, stop));
  assert(!zfbFooterStopPopulation(cap, { ...stop, known: [...stop.known, b(33, 30, 30)] }));
  assert(!zfbFooterStopPopulation(cap, { ...stop, steps: [] }));
});
