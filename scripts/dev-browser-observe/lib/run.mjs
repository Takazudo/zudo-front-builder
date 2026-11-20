import { existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { OBSERVER_PATH, launchOwnedBrowser } from "./browser.mjs";
import { runCalibration, CALIBRATION_CELLS } from "./calibrate.mjs";
import {
  adapterPath,
  loadFixture,
  materialize,
  originalLiveChecks,
  performWrite,
  prepareAttempt,
  restoreSnapshot,
  snapshotSources,
  warmPaths,
} from "./fixture.mjs";
import {
  ancestorNodeModules,
  collectIdentityFacts,
  evaluateIdentity,
  findLocalTakazudoPackages,
  harnessCapability,
} from "./identity.mjs";
import { LockBusyError, acquireLock } from "./lock.mjs";
import { hostIdentity, probeMac, sampledPeak } from "./mac-process-adapter.mjs";
import { liveOwnedFromRegistry, stopOwned } from "./process-owner.mjs";
import { LABEL_RE, loadProfile } from "./profile.mjs";
import { Recorder } from "./recorder.mjs";
import { startDevServer } from "./server.mjs";
import { newAccounting, runBatch } from "./state-machine.mjs";
import { summarize } from "./stats.mjs";
import {
  sha256File,
  appendJsonlSync,
  hrtimeNs,
  isoNow,
  nsToMs,
  readJson,
  readJsonl,
  writeJsonSync,
} from "./util.mjs";

export const EXIT = {
  ok: 0,
  stopped: 1,
  usage: 2,
  identityRejected: 3,
  notImplemented: 4,
  lockBusy: 5,
  ownedServerAlive: 6,
  serverNotReady: 7,
  controllerError: 8,
};

export const RECEIPT_SCHEMA = "dev-browser-observe.receipt.v1";

export function notImplementedResult(profile) {
  return {
    outcome: "not-implemented",
    mode: profile.mode,
    cell: "pending",
    message: "fixed-distribution mode is declared in the schema but not implemented",
  };
}

function plannedCount(profile) {
  return profile.scenarios.reduce((n, s) => n + s.ops.length, 0);
}

/** Identity verification only: no lock, no server, no materialization, no edits. */
export function verifyIdentityOnly(profilePath) {
  const { profile, resolved } = loadProfile(profilePath);
  const manifest = loadFixture(resolved.fixtureDir);
  let receipt = null;
  let receiptError = null;
  try {
    receipt = readJson(resolved.buildReceipt);
  } catch (error) {
    receiptError = String(error?.message ?? error);
  }
  const verdict = identityVerdict({
    profile,
    resolved,
    manifest,
    receipt,
    extraAppDirs: [resolved.outDir],
  });
  return { ...verdict, receiptError, harnessCapability: harnessCapability() };
}

function identityVerdict({ profile, resolved, manifest, receipt, extraAppDirs }) {
  const facts = collectIdentityFacts({
    receipt,
    appDirs: [join(resolved.fixtureDir, manifest.app), ...extraAppDirs],
    pinInputs: {
      fixtureDir: resolved.fixtureDir,
      observerPath: OBSERVER_PATH,
      adapterPath: adapterPath(resolved.fixtureDir, manifest, profile.fixture.adapter),
    },
  });
  const verdict = evaluateIdentity(profile, receipt, facts);
  const variant = manifest.adapters.variants[profile.fixture.adapter];
  const extra = [];
  if (variant?.sha256) {
    const actual = sha256File(adapterPath(resolved.fixtureDir, manifest, profile.fixture.adapter));
    extra.push({
      name: `adapter-provenance:${profile.fixture.adapter}`,
      ok: actual === variant.sha256,
      expected: variant.sha256,
      actual,
      provenance: variant.provenance,
    });
  }
  const gitTree = findGitWorkTree(resolved.outDir);
  extra.push({
    name: "outdir-outside-git-worktree",
    ok: gitTree === null,
    expected: null,
    actual: gitTree,
  });
  const readable = {
    name: "build-receipt-readable",
    ok: receipt !== null,
    expected: resolved.buildReceipt,
    actual: receipt ? "read" : null,
  };
  const checks = [readable, ...verdict.checks, ...extra];
  return { ok: checks.every((c) => c.ok), checks, facts };
}

export async function execute({
  profilePath,
  label,
  command,
  stderr = (m) => process.stderr.write(`${m}\n`),
}) {
  const loaded = loadProfile(profilePath);
  const { profile, resolved } = loaded;
  if (profile.mode === "fixed-distribution") {
    stderr(JSON.stringify(notImplementedResult(profile)));
    return EXIT.notImplemented;
  }
  if (!LABEL_RE.test(label ?? "")) {
    stderr(`invalid --label ${JSON.stringify(label)} (must match ${LABEL_RE})`);
    return EXIT.usage;
  }
  const gitTree = findGitWorkTree(resolved.outDir);
  if (gitTree) {
    stderr(
      `outDir ${resolved.outDir} is inside the git work tree ${gitTree}; use a directory outside any checkout`,
    );
    return EXIT.usage;
  }
  mkdirSync(resolved.outDir, { recursive: true });
  const labelDir = join(resolved.outDir, label);
  if (existsSync(labelDir)) {
    stderr(`label already used: ${labelDir} (records are never overwritten; pick a new label)`);
    return EXIT.usage;
  }

  let lock;
  try {
    lock = await acquireLock(resolved.lockPath);
  } catch (error) {
    stderr(
      error instanceof LockBusyError
        ? `another dev-browser-observe batch holds ${resolved.lockPath}`
        : String(error),
    );
    return EXIT.lockBusy;
  }
  try {
    const registry = readJsonl(resolved.registryPath).filter((e) => e.type === "spawn");
    const alive = liveOwnedFromRegistry(registry);
    if (alive.length > 0) {
      stderr(`an owned server from an earlier batch is still alive: ${JSON.stringify(alive)}`);
      return EXIT.ownedServerAlive;
    }
    mkdirSync(labelDir);
    return await executeLocked({ loaded, label, labelDir, command, stderr });
  } finally {
    await lock.release();
  }
}

async function executeLocked({ loaded, label, labelDir, command, stderr }) {
  const { profile, resolved } = loaded;
  const rec = new Recorder(join(labelDir, "records.jsonl"));
  const startedNs = hrtimeNs();
  const planned = command === "run" ? plannedCount(profile) : 0;
  rec.append({
    type: "run-start",
    command,
    label,
    profilePath: loaded.path,
    profileSha256: loaded.sha256,
  });

  const manifest = loadFixture(resolved.fixtureDir);
  let buildReceipt = null;
  try {
    buildReceipt = readJson(resolved.buildReceipt);
  } catch {
    buildReceipt = null;
  }
  const receipt = {
    schema: RECEIPT_SCHEMA,
    label,
    command,
    createdAt: isoNow(),
    productSha: profile.productSha,
    productDirtyIdentity: profile.productDirtyIdentity ?? null,
    diagnostic: profile.diagnostic === true,
    restorationWriteMethod: profile.restoration.writeMethod,
    harnessCapability: harnessCapability(),
    buildReceipt,
    identity: null,
    profile,
    profileSha256: loaded.sha256,
    fixture: null,
    browser: null,
    server: null,
    host: hostIdentity(),
    macProbe: probeMac(),
  };
  const receiptPath = join(labelDir, "receipt.json");
  const summaryPath = join(labelDir, "summary.json");

  const verdict = identityVerdict({
    profile,
    resolved,
    manifest,
    receipt: buildReceipt,
    extraAppDirs: [labelDir],
  });
  receipt.identity = verdict;
  rec.append({
    type: "identity",
    ok: verdict.ok,
    outcome: verdict.ok ? undefined : "identity-rejected",
    checks: verdict.checks,
  });
  if (!verdict.ok) {
    writeJsonSync(receiptPath, receipt);
    const acc = { ...newAccounting(planned), stopReason: "identity-rejected", error: 1 };
    writeJsonSync(summaryPath, { ...summarize(rec.records, acc), identityRejected: true });
    rec.append({ type: "run-end", exitCode: EXIT.identityRejected });
    stderr("identity rejected before any workload edit; see receipt.json identity.checks");
    return EXIT.identityRejected;
  }

  const m = materialize({
    fixtureDir: resolved.fixtureDir,
    workDir: join(labelDir, "work"),
    placement: profile.fixture.placement,
    adapter: profile.fixture.adapter,
    trailingSlash: profile.fixture.trailingSlash === true,
  });
  const local = [
    ...findLocalTakazudoPackages(m.appDir),
    ...findLocalTakazudoPackages(m.contentDir),
  ];
  receipt.fixture = {
    name: manifest.name,
    placement: profile.fixture.placement,
    adapter: profile.fixture.adapter,
    adapterLabel: manifest.adapters.variants[profile.fixture.adapter].label,
    adapterProvenance: manifest.adapters.variants[profile.fixture.adapter].provenance ?? null,
    adapterSourceSha256: manifest.adapters.variants[profile.fixture.adapter].sha256 ?? null,
    trailingSlash: m.trailingSlash,
    workDir: m.workDir,
    appDir: m.appDir,
    contentDir: m.contentDir,
    configSha256: m.configSha256,
    adapterSha256: m.adapterSha256,
    materializedTreeSha256: m.materializedTreeSha256,
    managedCss: manifest.css,
    localTakazudoPackages: local,
    ancestorNodeModules: ancestorNodeModules(m.appDir),
  };
  if (local.length > 0) {
    receipt.identity = {
      ...verdict,
      ok: false,
      checks: [
        ...verdict.checks,
        { name: "no-local-takazudo-packages:materialized", ok: false, expected: [], actual: local },
      ],
    };
    writeJsonSync(receiptPath, receipt);
    rec.append({
      type: "identity",
      ok: false,
      outcome: "identity-rejected",
      checks: receipt.identity.checks,
    });
    writeJsonSync(summaryPath, {
      ...summarize(rec.records, {
        ...newAccounting(planned),
        stopReason: "identity-rejected",
        error: 1,
      }),
      identityRejected: true,
    });
    rec.append({ type: "run-end", exitCode: EXIT.identityRejected });
    return EXIT.identityRejected;
  }

  const server = startDevServer({
    exe: buildReceipt.exe.path,
    appDir: m.appDir,
    port: profile.server.port,
    host: profile.server.host,
    extraArgs: profile.server.extraArgs ?? [],
    env: profile.server.env ?? {},
    diagnostic: profile.diagnostic === true,
    readyTimeoutMs: profile.server.readyTimeoutMs,
    logPath: join(labelDir, "server.log"),
  });
  appendJsonlSync(resolved.registryPath, {
    type: "spawn",
    label,
    at: isoNow(),
    ...server.handle.native,
  });
  receipt.server = {
    launcher: server.handle.launcher,
    native: server.handle.native,
    args: server.args,
    cwd: m.appDir,
    envOverrides: server.envOverrides,
    zfbEnv: server.zfbEnv,
    removedDiagnosticEnv: server.removedDiagnosticEnv,
    diagnostic: server.diagnostic,
  };

  let browserHandle = null;
  const samples = [];
  let exitCode = EXIT.ok;
  let summaryExtra = {};
  try {
    let ready;
    try {
      ready = await server.ready;
    } catch (error) {
      rec.append({
        type: "server-ready",
        outcome: "ready-failed",
        error: String(error?.message ?? error),
        exit: server.handle.exit,
      });
      exitCode = EXIT.serverNotReady;
      summaryExtra = { readyFailed: true };
      return exitCode;
    }
    rec.append({
      type: "server-ready",
      baseUrl: ready.baseUrl,
      readyMs: nsToMs(ready.readyNs - server.startedNs),
      native: server.handle.native,
      launcher: "none",
    });
    receipt.server.baseUrl = ready.baseUrl;
    receipt.server.readyMs = nsToMs(ready.readyNs - server.startedNs);

    browserHandle = await launchOwnedBrowser(profile.browser);
    const observer = browserHandle.observer;
    observer.baseUrl = ready.baseUrl;
    observer.onStray = (payload) =>
      rec.append({ type: "late-token", label: "not-success", armedToken: null, payload });
    receipt.browser = browserHandle.identity;
    rec.append({ type: "browser-ready", browser: browserHandle.identity });
    writeJsonSync(receiptPath, receipt);

    const sampleServer = () => {
      const s = server.sampleResources();
      samples.push(s);
      return s;
    };

    const fixtureOps = {
      prepareAttempt: (op) => prepareAttempt(m, op),
      performWrite: (w, onT0) => performWrite(w, onT0),
      snapshot: () => snapshotSources(m),
      restore: (s) => restoreSnapshot(s, { writeMethod: profile.restoration.writeMethod }),
      liveChecks: (s) => originalLiveChecks(m, s),
      warmPaths: (scenario) => warmPaths(m, scenario),
      route: m.route,
    };

    if (command === "run") {
      const { accounting, stopped } = await runBatch({
        scenarios: profile.scenarios,
        deps: {
          recorder: rec,
          clock: { now: hrtimeNs },
          server: { ...server, sampleResources: sampleServer },
          page: observer,
          fs: fixtureOps,
          limits: profile.limits,
          timeouts: profile.timeouts,
          startedNs,
        },
      });
      summaryExtra = { accountingFinal: accounting };
      exitCode =
        !stopped ||
        ((accounting.stoppedByExpectedControl || accounting.stoppedByClassifyTimeout) &&
          accounting.error === 0)
          ? EXIT.ok
          : EXIT.stopped;
    } else {
      const cal = profile.calibration ?? {};
      const results = await runCalibration({
        observer,
        append: (r) => rec.append(r),
        cells: cal.cells ?? CALIBRATION_CELLS,
        repeats: cal.repeats ?? 5,
        timeoutMs: profile.timeouts.attemptMs,
        staleTimeoutMs: cal.staleTimeoutMs ?? 2000,
        pagePath: m.route("independent"),
        fixtureOps,
        restoreMs: profile.timeouts.restoreMs,
        quietMs: profile.timeouts.quietMs ?? 1500,
      });
      sampleServer();
      summaryExtra = { calibration: results };
      exitCode = Object.values(results).every((r) => r.failures === 0) ? EXIT.ok : EXIT.stopped;
    }
    return exitCode;
  } catch (error) {
    rec.append({ type: "controller-error", error: String(error?.stack ?? error) });
    exitCode = EXIT.controllerError;
    return exitCode;
  } finally {
    if (browserHandle) {
      try {
        await browserHandle.context.close();
        await browserHandle.browser.close();
      } catch (error) {
        rec.append({
          type: "controller-error",
          phase: "browser-close",
          error: String(error?.message ?? error),
        });
      }
    }
    let stopReport;
    try {
      stopReport = await stopOwned(server.handle, {
        intWaitMs: profile.timeouts.stopTermMs ?? 5000,
        termWaitMs: profile.timeouts.stopKillMs ?? 3000,
        killWaitMs: 3000,
      });
    } catch (error) {
      stopReport = {
        absent: false,
        error: String(error?.message ?? error),
        code: error?.code ?? null,
      };
    }
    await new Promise((r) => server.log.end(r));
    rec.append({ type: "server-stop", report: stopReport, exit: server.handle.exit });
    appendJsonlSync(resolved.registryPath, {
      type: "stopped",
      label,
      at: isoNow(),
      pid: server.handle.native?.pid,
      absent: stopReport.absent,
    });
    const accounting = summaryExtra.accountingFinal ?? {
      ...newAccounting(planned),
      stopReason: summaryExtra.readyFailed
        ? "ready-failed"
        : exitCode === EXIT.controllerError
          ? "controller-error"
          : null,
    };
    if (summaryExtra.readyFailed || exitCode === EXIT.controllerError) accounting.error += 1;
    const summary = {
      ...summarize(rec.records, accounting),
      command,
      label,
      exitCode,
      calibration: summaryExtra.calibration ?? null,
      resourceSamples: { count: samples.length, peak: sampledPeak(samples) },
      ownedServerAbsentAfterStop: Boolean(stopReport.absent),
      serverExit: server.handle.exit,
      wallMs: nsToMs(hrtimeNs() - startedNs),
    };
    writeJsonSync(summaryPath, summary);
    writeJsonSync(receiptPath, receipt);
    rec.append({ type: "run-end", exitCode });
    if (!stopReport.absent)
      stderr("owned server processes survived stop; the next batch will refuse to start");
  }
}

/** The nearest enclosing git work tree root (a dir holding `.git`), or null. */
export function findGitWorkTree(dir) {
  let cur = resolve(dir);
  for (;;) {
    if (existsSync(join(cur, ".git"))) return cur;
    const parent = dirname(cur);
    if (parent === cur) return null;
    cur = parent;
  }
}
