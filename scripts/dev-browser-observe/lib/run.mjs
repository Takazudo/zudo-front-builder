import { existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { OBSERVER_PATH, closeOwnedBrowser, launchOwnedBrowser } from "./browser.mjs";
import { runCalibration, CALIBRATION_CELLS } from "./calibrate.mjs";
import { kindFor } from "./fixture-kinds.mjs";
import { resolveLazyDevRender, traceEnabledIn } from "./provenance.mjs";
import {
  ancestorNodeModules,
  collectIdentityFacts,
  evaluateIdentity,
  findLocalTakazudoPackages,
  harnessCapability,
} from "./identity.mjs";
import { LockBusyError, acquireLock } from "./lock.mjs";
import { hostIdentity, probeMac, sampledPeak } from "./mac-process-adapter.mjs";
import { liveOwnedFromRegistry, shouldRegisterOwned, stopOwned } from "./process-owner.mjs";
import { LABEL_RE, loadProfile } from "./profile.mjs";
import { Recorder } from "./recorder.mjs";
import { startDevServer } from "./server.mjs";
import { batchExitOk, newAccounting, runBatch } from "./state-machine.mjs";
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

export const BROWSER_CLOSE_TIMEOUT_MS = 10000;

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
  const manifest = kindFor(profile).loadManifest(resolved.fixtureDir);
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
  const kind = kindFor(profile);
  const adapterFile = kind.adapterPath(resolved.fixtureDir, manifest, profile.fixture.adapter);
  const facts = collectIdentityFacts({
    receipt,
    appDirs: [...kind.appDirs(resolved.fixtureDir, manifest, profile, resolved), ...extraAppDirs],
    pinInputs: {
      fixtureDir: resolved.fixtureDir,
      observerPath: OBSERVER_PATH,
      adapterPath: adapterFile,
    },
  });
  const verdict = evaluateIdentity(profile, receipt, facts);
  const variant = kind.adapterVariant(manifest, profile.fixture.adapter);
  const extra = [];
  if (variant?.sha256) {
    const actual = sha256File(adapterFile);
    extra.push({
      name: `adapter-provenance:${profile.fixture.adapter}`,
      ok: actual === variant.sha256,
      expected: variant.sha256,
      actual,
      provenance: variant.provenance,
    });
  }
  extra.push(...kind.extraChecks(profile, resolved, manifest));
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
  hooks = {},
  scenarios = null,
}) {
  const loaded = loadProfile(profilePath);
  const { profile, resolved } = loaded;
  if (scenarios) {
    const known = new Set(profile.scenarios.map((s) => s.id));
    const unknown = scenarios.filter((id) => !known.has(id));
    if (unknown.length > 0 || scenarios.length === 0) {
      stderr(`unknown --scenario ${JSON.stringify(unknown)}; known: ${[...known].join(", ")}`);
      return EXIT.usage;
    }
    loaded.scenarioSelection = scenarios;
    loaded.profile = {
      ...profile,
      scenarios: profile.scenarios.filter((s) => scenarios.includes(s.id)),
    };
    return execute2({ loaded, label, command, stderr, hooks });
  }
  return execute2({ loaded, label, command, stderr, hooks });
}

async function execute2({ loaded, label, command, stderr, hooks }) {
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
    if (error instanceof LockBusyError) {
      stderr(`another dev-browser-observe batch holds ${resolved.lockPath}`);
      return EXIT.lockBusy;
    }
    stderr(`lock acquisition failed (not contention): ${error?.stack ?? error}`);
    return EXIT.controllerError;
  }
  try {
    const registry = readJsonl(resolved.registryPath).filter(
      (e) => e.type === "spawn" || e.type === "owned",
    );
    const alive = liveOwnedFromRegistry(registry);
    if (alive.length > 0) {
      stderr(`an owned server from an earlier batch is still alive: ${JSON.stringify(alive)}`);
      return EXIT.ownedServerAlive;
    }
    try {
      mkdirSync(labelDir);
    } catch (error) {
      if (error?.code === "EEXIST") {
        stderr(`label already used: ${labelDir} (created by another process; pick a new label)`);
        return EXIT.usage;
      }
      throw error;
    }
    return await executeLocked({ loaded, label, labelDir, command, stderr, hooks });
  } finally {
    await lock.release();
  }
}

async function executeLocked({ loaded, label, labelDir, command, stderr, hooks = {} }) {
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
    scenarioSelection: loaded.scenarioSelection ?? null,
  });

  const kind = kindFor(profile);
  const manifest = kind.loadManifest(resolved.fixtureDir);
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
    scenarioSelection: loaded.scenarioSelection ?? null,
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

  let m = null;
  let server = null;
  let browserHandle = null;
  const samples = [];
  let exitCode = EXIT.ok;
  let summaryExtra = {};
  const accountingRef = { current: null };
  try {
    try {
      m = kind.materialize({ profile, resolved, workDir: join(labelDir, "work") });
    } catch (error) {
      rec.append({
        type: "controller-error",
        phase: "materialize",
        outcome: "materialize-error",
        error: String(error?.stack ?? error),
      });
      summaryExtra = { stopReason: "materialize-error" };
      exitCode = EXIT.controllerError;
      return exitCode;
    }
    const local = [
      ...findLocalTakazudoPackages(m.appDir),
      ...findLocalTakazudoPackages(m.contentDir),
    ];
    receipt.fixture = {
      ...kind.receiptFixture(m, manifest, profile),
      localTakazudoPackages: local,
      ancestorNodeModules: ancestorNodeModules(m.appDir),
    };
    if (local.length > 0) {
      receipt.identity = {
        ...verdict,
        ok: false,
        checks: [
          ...verdict.checks,
          {
            name: "no-local-takazudo-packages:materialized",
            ok: false,
            expected: [],
            actual: local,
          },
        ],
      };
      rec.append({
        type: "identity",
        ok: false,
        outcome: "identity-rejected",
        checks: receipt.identity.checks,
      });
      summaryExtra = { stopReason: "identity-rejected", identityRejected: true };
      exitCode = EXIT.identityRejected;
      return exitCode;
    }

    server = startDevServer({
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
    server.handle.onNewOwned = (p) => {
      if (shouldRegisterOwned(server.handle.native, p))
        appendJsonlSync(resolved.registryPath, { type: "owned", label, at: isoNow(), ...p });
    };
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
      summaryExtra = { stopReason: "ready-failed" };
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

    browserHandle = await (hooks.launchOwnedBrowser ?? launchOwnedBrowser)(profile.browser);
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

    const fixtureOps = kind.ops(m, profile);

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
          accountingRef,
          provenance:
            profile.diagnostic === true
              ? {
                  traceEnabled: traceEnabledIn(server.zfbEnv),
                  renderMode: resolveLazyDevRender(server.zfbEnv),
                  pageFor: fixtureOps.pageFor ?? null,
                }
              : null,
        },
      });
      summaryExtra = { accountingFinal: accounting };
      exitCode = batchExitOk(accounting, stopped) ? EXIT.ok : EXIT.stopped;
    } else {
      const cal = profile.calibration ?? {};
      const results = await runCalibration({
        observer,
        append: (r) => rec.append(r),
        cells: cal.cells ?? CALIBRATION_CELLS,
        repeats: cal.repeats ?? 5,
        timeoutMs: profile.timeouts.attemptMs,
        staleTimeoutMs: cal.staleTimeoutMs ?? 2000,
        pagePath: kind.calibrationPage(m),
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
    rec.append({
      type: "controller-error",
      outcome: "controller-error",
      error: String(error?.stack ?? error),
    });
    exitCode = EXIT.controllerError;
    summaryExtra = { ...summaryExtra, stopReason: summaryExtra.stopReason ?? "controller-error" };
    return exitCode;
  } finally {
    // Best-effort from here on: a failing record write must never skip the server stop or the summary.
    const safeAppend = (r) => {
      try {
        rec.append(r);
      } catch (error) {
        stderr(`record write failed (${r.type}): ${error?.message ?? error}`);
      }
    };
    let stopReport = { absent: true, reason: "no-server-started" };
    if (server) {
      try {
        stopReport = await (hooks.stopOwned ?? stopOwned)(server.handle, {
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
      let logClose = null;
      try {
        logClose = await server.closeLog();
      } catch (error) {
        logClose = { error: String(error?.message ?? error) };
      }
      safeAppend({ type: "server-stop", report: stopReport, exit: server.handle.exit, logClose });
      try {
        appendJsonlSync(resolved.registryPath, {
          type: "stopped",
          label,
          at: isoNow(),
          pid: server.handle.native?.pid,
          absent: stopReport.absent,
        });
      } catch (error) {
        stderr(`registry write failed: ${error?.message ?? error}`);
      }
    }
    if (browserHandle) {
      try {
        for (const e of await (hooks.closeOwnedBrowser ?? closeOwnedBrowser)(browserHandle, {
          timeoutMs: BROWSER_CLOSE_TIMEOUT_MS,
        }))
          safeAppend({
            type: "controller-error",
            phase: e.step,
            error: e.error,
            timedOut: e.timedOut ?? false,
          });
      } catch (error) {
        safeAppend({
          type: "controller-error",
          phase: "browser-close",
          error: String(error?.message ?? error),
        });
      }
    }
    let stopFailure = null;
    if (server && (!stopReport.absent || stopReport.error)) {
      stopFailure = {
        absent: Boolean(stopReport.absent),
        error: stopReport.error ?? null,
        code: stopReport.code ?? null,
        exitBefore: exitCode,
      };
      exitCode = EXIT.ownedServerAlive;
    }
    const live = summaryExtra.accountingFinal ?? accountingRef.current;
    const accounting = live ? { ...live } : { ...newAccounting(planned) };
    if (!summaryExtra.accountingFinal && summaryExtra.stopReason) {
      accounting.stopReason = accounting.stopReason ?? summaryExtra.stopReason;
      accounting.error += 1;
    }
    try {
      const summary = {
        ...summarize(rec.records, accounting),
        accountingSource: summaryExtra.accountingFinal
          ? "batch-result"
          : live
            ? "live-batch-accounting"
            : "planned",
        command,
        label,
        exitCode,
        identityRejected: summaryExtra.identityRejected === true,
        calibration: summaryExtra.calibration ?? null,
        resourceSamples: { count: samples.length, peak: sampledPeak(samples) },
        ownedServerAbsentAfterStop: Boolean(stopReport.absent),
        stopFailure,
        serverExit: server?.handle.exit ?? null,
        wallMs: nsToMs(hrtimeNs() - startedNs),
      };
      writeJsonSync(summaryPath, summary);
    } catch (error) {
      stderr(`summary write failed: ${error?.message ?? error}`);
    }
    try {
      writeJsonSync(receiptPath, receipt);
    } catch (error) {
      stderr(`receipt write failed: ${error?.message ?? error}`);
    }
    safeAppend({ type: "run-end", exitCode });
    if (stopFailure) {
      stderr(
        "owned server processes were not verified absent after stop; the next batch will refuse to start",
      );
      // Returning from finally deliberately overrides the try block's exit code.
      return exitCode; // eslint-disable-line no-unsafe-finally
    }
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
