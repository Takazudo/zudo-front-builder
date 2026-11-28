import assert from "node:assert/strict";
import { closeSync, readlinkSync } from "node:fs";
import { linuxBirth } from "./linux-birth.mjs";
import {
  nextOuterChildren,
  readNextOuterProductGraph,
  verifyNextOuterWiring,
  verifyNextOuterProvider,
} from "./next-outer-provider.mjs";
import { nextOuterNonceDigest, observeNextOuterReceipt } from "./next-outer-channel.mjs";
import {
  nextOuterBatchHooks,
  nextOuterOutcomeStatus,
  outerTerminalGate,
} from "./next-outer-terminal.mjs";
import { startSourceTimingSafety, CSEC_EXE_SHA256 } from "./source-timing-safety.mjs";
import { samplerClosureAudit } from "./linux-rss-sample.mjs";
import { startFixedCustody } from "./fixed-rss-custody.mjs";
import {
  markNextRegistry,
  readNextRegistry,
  reconcileNextPopulation,
} from "./next-peer-population.mjs";
import { interruptible } from "./session-deadline.mjs";
import { admissionPath, reserveSession } from "./admission.mjs";
import { existsSync, mkdirSync, realpathSync } from "node:fs";
import { open } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import {
  OBSERVER_PATH,
  closeOwnedBrowser,
  fixedPeerBrowserImageErrors,
  launchOwnedBrowser,
} from "./browser.mjs";
import { BrowserTiming, timeBrowserAsync, timeBrowserSync } from "./browser-timing.mjs";
import { runCalibration, CALIBRATION_CELLS } from "./calibrate.mjs";
import { kindFor } from "./fixture-kinds.mjs";
import { resolveLazyDevRender, traceEnabledIn } from "./provenance.mjs";
import {
  ancestorNodeModules,
  collectIdentityFacts,
  evaluateIdentity,
  findLocalTakazudoPackages,
  frozenIdentityChecks,
  harnessCapability,
} from "./identity.mjs";
import { LockBusyError, acquireLock } from "./lock.mjs";
import { hostIdentity, probeMac, sampledPeak } from "./mac-process-adapter.mjs";
import { liveOwnedFromRegistry, shouldRegisterOwned, stopOwned } from "./process-owner.mjs";
import { LABEL_RE, loadProfile } from "./profile.mjs";
import { ControllerTiming, timed } from "./controller-timing.mjs";
import { Recorder } from "./recorder.mjs";
import { createRendererMemoryStudy } from "./renderer-memory-study.mjs";
import {
  rendererMemoryAllocatorProtocol,
  LINUX_RSS_SAMPLER_VERSION,
  RENDERER_MEMORY_LIMITS,
  RENDERER_MEMORY_MAPPING_LIMITS,
  RENDERER_MEMORY_RECLAMATION_PROTOCOL,
  RENDERER_MEMORY_RECLAMATION_LIMITS,
} from "./renderer-memory-protocol.mjs";
import { createResourceStudy, summarizeResourceStudy } from "./resource-study.mjs";
import { checkStartupArticles, verifyStartup, verifyStartupSample } from "./startup-acceptance.mjs";
import { StartupEventLoop } from "./startup-event-loop.mjs";
import { expandSchedule, scheduleOf } from "./schedule.mjs";
import { startDevServer } from "./server.mjs";
import {
  appTreeHash,
  classifyRoles,
  createReadiness,
  launchCommand,
  materializeFixed,
  processIdentityChecks,
  resolveNpmCli,
} from "./distribution.mjs";
import {
  FIXED_ASTRO_FOOTER_PROTOCOL,
  FIXED_ASTRO_TITLE_PROTOCOL,
  FIXED_ASTRO_CATEGORY_PROTOCOL,
  FIXED_PEER_PROTOCOL,
  readPeerRoles,
  isFixedAstroProtocol,
  verifyAstroPopulation,
} from "./fixed-peer-roles.mjs";
import { buildNextBootstrapContract, NEXT_BOOTSTRAP_PROTOCOL } from "./fixed-next-bootstrap.mjs";
import { startNextPeerCustody } from "./next-peer-custody.mjs";
import { FN_SEL } from "./fieldnotes.mjs";
import {
  fixedContextSample,
  fixedPeerExecutables,
  fixedZfbExecutables,
  startFixedRssSafety,
} from "./fixed-rss-safety.mjs";
import { fixedIdentityVerdict, freePort } from "./fixed.mjs";
import { snapshotOwned, snapshotOwnedWithArgs } from "./process-owner.mjs";
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

/** Exit code of a finished run batch, shared by executeLocked and its tests. */
export function batchExitCode({
  lateFailure,
  studyFailure,
  rendererFailure,
  accounting,
  stopped,
  fixedPeer,
}) {
  if (lateFailure) return EXIT.identityRejected;
  if (studyFailure || rendererFailure) return EXIT.controllerError;
  return batchExitOk(accounting, stopped) && (!fixedPeer || accounting.success === 1)
    ? EXIT.ok
    : EXIT.stopped;
}

/**
 * The v4 outer cell's next-outer-outcome.json body (less its bindings), shared
 * by executeLocked and its tests. `refusals` counts the outer refusals by stage,
 * so the pre-truncate count can be judged against the fresh margin.
 */
export function nextOuterRunOutcome({
  stopReport,
  stopFailure,
  unresolved,
  evidenceFailed,
  population,
  safetyFailure,
  operation,
  records,
  exitCode,
  accounting,
  outerDecision,
  refusals,
}) {
  const closed = Boolean(stopReport?.absent && !stopFailure && unresolved.length === 0);
  const proofComplete = Boolean(!evidenceFailed && population?.ok && !safetyFailure);
  const { status, classification = null } = nextOuterOutcomeStatus({
    operation,
    records,
    closed,
    proofComplete,
    exitCode,
    exit: EXIT,
    accounting,
    outerDecision,
  });
  return {
    closureVerified: closed,
    proofComplete,
    status,
    classification,
    refusals: { ...refusals },
  };
}

export const RECEIPT_SCHEMA = "dev-browser-observe.receipt.v1";
const BROWSER_STARTUP_TIMING_FILE = "browser-startup-timing.json";

export function notImplementedResult(profile) {
  return {
    outcome: "not-implemented",
    mode: profile.mode,
    cell: "pending",
    message: "fixed-distribution mode is declared in the schema but not implemented",
  };
}

export const isFixed = (profile) => profile.mode === "fixed-distribution";

function plannedCount(scenarios) {
  return scenarios.reduce((n, s) => n + s.ops.length, 0);
}

/** Identity verification only: no lock, no server, no materialization, no edits. */
export function verifyIdentityOnly(profilePath) {
  const { profile, resolved } = loadProfile(profilePath);
  const manifest = kindFor(profile).loadManifest(resolved.fixtureDir);
  if (isFixed(profile)) {
    const v = fixedIdentityVerdict({ profile, resolved, manifest });
    return { ...v, receiptError: null, harnessCapability: harnessCapability() };
  }
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
  if (profile.sourceTimingSafety) {
    extra.push({
      name: "source-timing-csec-binary",
      expected: CSEC_EXE_SHA256,
      actual: receipt?.exe?.sha256 ?? null,
      ok: receipt?.exe?.sha256 === CSEC_EXE_SHA256,
    });
    extra.push({
      name: "source-timing-csec-build",
      expected: "clean locked C-sec dev default rustc 1.99.1, 64 embedded members",
      ok:
        receipt?.source?.dirty === false &&
        receipt?.toolchain?.rustc?.release === "1.99.1" &&
        receipt?.cargoLockSha256 ===
          "804d5b831930816471fc101bd5c4ab390ccd1861d73808e046b4bfedb22c1dc4" &&
        receipt?.embeddedBytes?.checked === 64 &&
        receipt.embeddedBytes.missing?.length === 0 &&
        receipt?.build?.profile === "dev" &&
        receipt.build.noDefaultFeatures === false &&
        receipt.build.features?.length === 0,
    });
  }
  if (profile.rendererMemoryStudy) {
    const { browserExecutable, browserExecutableSha256 } = profile.rendererMemoryStudy;
    let actual = null;
    try {
      actual = sha256File(realpathSync(browserExecutable));
    } catch (error) {
      actual = { error: String(error?.message ?? error) };
    }
    extra.push({
      name: "renderer-memory-browser-executable",
      ok: actual === browserExecutableSha256,
      expected: { path: browserExecutable, sha256: browserExecutableSha256 },
      actual,
    });
  }
  extra.push(...kind.extraChecks(profile, resolved, manifest));
  extra.push(...frozenIdentityChecks());
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
  signal,
  onFinalizing,
  supervisorId,
  nextOuter,
  supervisorStartedMs,
  workDeadline,
  supervisorDeadlineMs,
  scenarios = null,
}) {
  const loaded = loadProfile(profilePath);
  loaded.signal = signal;
  loaded.nextOuter = nextOuter;
  loaded.onFinalizing = onFinalizing;
  loaded.supervisorId = supervisorId;
  loaded.supervisorStartedMs = supervisorStartedMs;
  loaded.workDeadline = workDeadline;
  loaded.supervisorDeadlineMs = supervisorDeadlineMs;
  const { profile, resolved } = loaded;
  const startup = Boolean(profile.startupAcceptance);
  const tiny = Boolean(profile.tinyAcceptance);
  if ((command === "startup-acceptance") !== startup || (command === "tiny-acceptance") !== tiny) {
    stderr("acceptance requires its own profile and command");
    return EXIT.usage;
  }
  if (
    (profile.rendererMemoryStudy ||
      startup ||
      tiny ||
      profile.fixedRssSafety ||
      profile.sourceTimingSafety) &&
    (command !== (tiny ? "tiny-acceptance" : startup ? "startup-acceptance" : "run") ||
      scenarios !== null ||
      process.platform !== "linux" ||
      !supervisorId ||
      (profile.sourceTimingSafety &&
        (workDeadline !== supervisorDeadlineMs - 60000 ||
          supervisorDeadlineMs - supervisorStartedMs !== profile.limits.wallCeilingMs)) ||
      ![supervisorStartedMs, workDeadline, supervisorDeadlineMs].every(Number.isSafeInteger))
  ) {
    stderr("renderer-memory diagnostics require a standalone Linux run with a supervisor");
    return EXIT.usage;
  }
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
  if (!LABEL_RE.test(label ?? "")) {
    stderr(`invalid --label ${JSON.stringify(label)} (must match ${LABEL_RE})`);
    return EXIT.usage;
  }
  try {
    loaded.runScenarios =
      command === "run" || profile.tinyAcceptance?.probe === "B"
        ? expandSchedule(profile)
        : profile.scenarios;
  } catch (error) {
    stderr(String(error?.message ?? error));
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

  const intentPath = process.platform === "linux" ? (hooks.admissionPath ?? admissionPath()) : null;
  if (intentPath && existsSync(intentPath)) {
    stderr(`unresolved benchmark session: inspect ${intentPath} before running another session`);
    return EXIT.ownedServerAlive;
  }
  let lock;
  try {
    lock = await acquireLock(resolved.lockPath, { signal: loaded.signal });
  } catch (error) {
    const record = {
      type: "lock-acquisition-failure",
      at: isoNow(),
      label,
      supervisorId: loaded.supervisorId ?? null,
      error: String(error.message),
      lockHelper: error.lockHelper ?? null,
    };
    const journal = join(resolved.outDir, ".dev-browser-observe-lock-failures.jsonl");
    try {
      appendJsonlSync(journal, record);
      stderr("lock acquisition failure receipt: " + journal);
    } catch (writeError) {
      stderr("lock failure receipt could not be written: " + writeError.message);
      stderr(JSON.stringify(record));
    }
    if (error instanceof LockBusyError) {
      stderr(`another dev-browser-observe batch holds ${resolved.lockPath}`);
      return EXIT.lockBusy;
    }
    stderr(`lock acquisition failed (not contention): ${error?.stack ?? error}`);
    return EXIT.controllerError;
  }
  try {
    const history = readJsonl(resolved.registryPath);
    const completed = new Set(
      history.filter((e) => e.type === "stopped" && e.absent === true).map((e) => e.label),
    );
    const registry = history.filter(
      (e) => !completed.has(e.label) && (e.type === "spawn" || e.type === "owned"),
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
    if (loaded.nextOuter) loaded.outerLock = linuxBirth(lock.helperPid);
    return await executeLocked({ loaded, label, labelDir, command, stderr, hooks, intentPath });
  } finally {
    await lock.release();
  }
}

// A bounded stop may reject before its original in-flight helper settles.
// At the existing finalization boundary, preserve any newly available identity
// and closure evidence without waiting again or releasing the original hold.
export function recordSamplerClosureAudits(unresolved, fixedSafety, rendererMemory, append) {
  for (const resource of unresolved) {
    if (!resource.helper) {
      const monitor = ["fixed-rss-sampler", "source-timing-sampler"].includes(resource.resource)
        ? fixedSafety
        : resource.resource === "renderer-memory-sampler"
          ? rendererMemory
          : null;
      const helper = monitor?.unresolvedHelper;
      if (helper) resource.helper = helper;
    }
    const closure = resource.helper && samplerClosureAudit(resource.helper);
    if (closure) {
      resource.closureAudit = closure;
      append({ type: "sampler-closure-audit", ...resource });
    }
  }
}

async function executeLocked({ loaded, label, labelDir, command, stderr, hooks = {}, intentPath }) {
  const { profile, resolved } = loaded;
  const fixedAstro = isFixedAstroProtocol(profile.fixedRssSafety);
  const decisionProtocol = profile.fixedRssSafety === "linux-fixed-peer.v3";
  const outerProtocol = profile.fixedRssSafety === "linux-fixed-peer.v4";
  const nextProtocol =
    outerProtocol || decisionProtocol || profile.fixedRssSafety === NEXT_BOOTSTRAP_PROTOCOL;
  const bodyPeer = nextProtocol || profile.fixedRssSafety === FIXED_PEER_PROTOCOL;
  const fixedPeer = fixedAstro || bodyPeer;
  let nextCustody = null;
  let outerReceiver, outerGate, outerDecision, outerCadence, outerWiring;
  const outerRefusals = { "prewrite-settle": 0, "prewrite-decision": 0, "pre-truncate": 0 };
  let holderSocketOwned = false;
  let nextRegistryMark;
  const sourceTiming = Boolean(profile.sourceTimingSafety);
  const fixedAbort = profile.fixedRssSafety || sourceTiming ? new AbortController() : null;
  if (fixedAbort)
    loaded.signal = loaded.signal
      ? AbortSignal.any([loaded.signal, fixedAbort.signal])
      : fixedAbort.signal;
  let fixedSafety = null;
  let fixedCustody = null;
  let stoppingServer;
  const stopCancellation = new AbortController();
  const stopServer = (handle) => {
    stoppingServer ??= (hooks.stopOwned ?? stopOwned)(handle, {
      signal: stopCancellation.signal,
      intWaitMs: profile.timeouts.stopTermMs ?? 5000,
      termWaitMs: profile.timeouts.stopKillMs ?? 3000,
      killWaitMs: 3000,
    });
    return stoppingServer;
  };
  const tiny = profile.tinyAcceptance;
  const timing =
    tiny || profile.rendererMemoryStudy || profile.startupAcceptance
      ? new ControllerTiming(hooks.rendererMemoryClock)
      : null;
  const browserTiming = tiny?.probe === "B" ? new BrowserTiming() : null;
  const startupLoop = browserTiming ? new StartupEventLoop() : null;
  const writeBrowserStartupTiming = () => {
    if (!browserTiming) return;
    writeJsonSync(join(labelDir, BROWSER_STARTUP_TIMING_FILE), {
      schema: "dev-browser-observe.browser-startup-timing.v1",
      browser: browserTiming.snapshot(),
      eventLoop: startupLoop.report,
    });
  };
  const rec = new Recorder(join(labelDir, "records.jsonl"), timing);
  const memoryProfile = profile.rendererMemoryStudy ?? null;
  const acceptance = profile.startupAcceptance ?? tiny;
  const acceptanceKey = tiny ? "tinyAcceptance" : "startupAcceptance";
  const batch = command === "run" || tiny?.probe === "B";
  const memoryDiagnostic = memoryProfile ?? acceptance;
  const startedNs = memoryDiagnostic ? BigInt(loaded.supervisorStartedMs) * 1_000_000n : hrtimeNs();
  const runScenarios = loaded.runScenarios ?? profile.scenarios;
  const schedule = batch ? scheduleOf(runScenarios) : [];
  const planned = batch ? plannedCount(runScenarios) : 0;
  rec.append({
    type: "run-start",
    command,
    label,
    profilePath: loaded.path,
    profileSha256: loaded.sha256,
    scenarioSelection: loaded.scenarioSelection ?? null,
    schedule,
    resourceStudy: command === "run" ? (profile.resourceStudy ?? null) : null,
    ...(profile.fixedRssSafety && { fixedRssSafety: profile.fixedRssSafety }),
    ...(sourceTiming && { sourceTimingSafety: profile.sourceTimingSafety }),
    ...(memoryProfile ? { diagnostic: true, rendererMemoryStudy: memoryProfile } : {}),
    ...(acceptance && { diagnostic: true, [acceptanceKey]: acceptance }),
  });

  const kind = kindFor(profile);
  const manifest = kind.loadManifest(resolved.fixtureDir);
  const fixed = isFixed(profile);
  let buildReceipt = null;
  if (!fixed)
    try {
      buildReceipt = readJson(resolved.buildReceipt);
    } catch {
      buildReceipt = null;
    }
  const receipt = {
    schema: RECEIPT_SCHEMA,
    ...(loaded.supervisorId ? { supervisorId: loaded.supervisorId } : {}),
    label,
    command,
    createdAt: isoNow(),
    mode: profile.mode,
    framework: fixed ? profile.framework : "zfb-source-built",
    productSha: profile.productSha ?? null,
    productDirtyIdentity: profile.productDirtyIdentity ?? null,
    diagnostic: profile.diagnostic === true,
    ...(profile.fixedRssSafety && { fixedRssSafety: profile.fixedRssSafety }),
    ...(sourceTiming && { sourceTimingSafety: profile.sourceTimingSafety }),
    ...(timing && { controllerTiming: "controller-timing.json" }),
    ...(acceptance && {
      [acceptanceKey]: {
        ...acceptance,
        samplerVersion: LINUX_RSS_SAMPLER_VERSION,
        complete: false,
        timingEligible: false,
        studyEligible: false,
        ...(browserTiming && { browserStartupTiming: BROWSER_STARTUP_TIMING_FILE }),
        supervisorStartedMs: loaded.supervisorStartedMs,
        workDeadlineMs: loaded.workDeadline,
        supervisorDeadlineMs: loaded.supervisorDeadlineMs,
      },
    }),
    ...(memoryProfile
      ? {
          rendererMemoryStudy: {
            ...memoryProfile,
            samplerVersion: LINUX_RSS_SAMPLER_VERSION,
            limits: RENDERER_MEMORY_LIMITS,
            ...(rendererMemoryAllocatorProtocol(memoryProfile.protocol) && {
              mappingLimits: RENDERER_MEMORY_MAPPING_LIMITS,
            }),
            ...(memoryProfile.protocol === RENDERER_MEMORY_RECLAMATION_PROTOCOL && {
              reclamationLimits: RENDERER_MEMORY_RECLAMATION_LIMITS,
            }),
            supervisorStartedMs: loaded.supervisorStartedMs,
            workDeadlineMs: loaded.workDeadline,
            supervisorDeadlineMs: loaded.supervisorDeadlineMs,
            clock:
              "Linux process.hrtime.bigint (monotonic nanoseconds); supervisor deadlines in floored milliseconds",
            node: {
              version: process.version,
              path: realpathSync(process.execPath),
              sha256: sha256File(process.execPath),
            },
          },
        }
      : {}),
    restorationWriteMethod: profile.restoration.writeMethod,
    harnessCapability: harnessCapability(),
    buildReceipt,
    distribution: null,
    identity: null,
    profile,
    profileSha256: loaded.sha256,
    scenarioSelection: loaded.scenarioSelection ?? null,
    schedule,
    fixture: null,
    browser: null,
    server: null,
    host: hostIdentity(),
    macProbe: probeMac(),
  };
  const receiptPath = join(labelDir, "receipt.json");
  const summaryPath = join(labelDir, "summary.json");

  const verdict = fixed
    ? (hooks.fixedIdentityVerdict ?? fixedIdentityVerdict)({ profile, resolved, manifest })
    : identityVerdict({
        profile,
        resolved,
        manifest,
        receipt: buildReceipt,
        extraAppDirs: [labelDir],
      });
  receipt.identity = verdict;
  if (fixed) receipt.distribution = verdict.facts ?? null;
  rec.append({
    type: "identity",
    ok: verdict.ok,
    outcome: verdict.ok ? undefined : "identity-rejected",
    checks: verdict.checks,
  });
  if (!verdict.ok) {
    writeJsonSync(receiptPath, receipt);
    const acc = { ...newAccounting(planned), stopReason: "identity-rejected", error: 1 };
    writeJsonSync(summaryPath, {
      ...summarize(rec.records, acc),
      identityRejected: true,
      ...(acceptance && { diagnostic: true, [acceptanceKey]: receipt[acceptanceKey] }),
      ...(memoryProfile
        ? {
            diagnostic: true,
            rendererMemoryStudy: {
              protocol: memoryProfile.protocol,
              reclamationArm: memoryProfile.reclamationArm,
              scheduleComplete: false,
              observationComplete: false,
            },
          }
        : {}),
    });
    rec.append({ type: "run-end", exitCode: EXIT.identityRejected });
    if (timing) writeJsonSync(join(labelDir, "controller-timing.json"), timing.report);
    writeBrowserStartupTiming();
    stderr("identity rejected before any workload edit; see receipt.json identity.checks");
    return EXIT.identityRejected;
  }

  let m = null;
  let server = null;
  let intent = null;
  let spawning = false;
  let browserHandle = null;
  const samples = [];
  let study = null;
  let rendererMemory = null;
  let mappingCollector = null;
  let memoryReport = null;
  let logClose = null;
  let checkLateProcesses = null;
  let verifyAstroCurrent;
  let lateFailure = null;
  let exitCode = EXIT.ok;
  let summaryExtra = {};
  const accountingRef = { current: null };
  try {
    if (outerProtocol) {
      const decl = loaded.nextOuter;
      assert(
        decl && Array.isArray(decl.sockets) && decl.sockets.length === 3,
        "missing supervised outer channel",
      );
      assert.deepEqual(
        [4, 5, 6].map((fd) => readlinkSync("/proc/self/fd/" + fd)),
        decl.sockets,
      );
      holderSocketOwned = true;
      const deadlineNs = BigInt(loaded.workDeadline) * 1000000n;
      try {
        receipt.outerProvider = await verifyNextOuterProvider({
          profile: profile.fixedNextOuterObserver,
          decl,
          profilePath: loaded.path,
          label,
          deadlineNs,
          signal: loaded.signal,
        });
      } catch (error) {
        try {
          closeSync(5);
        } finally {
          closeSync(6);
        }
        throw error;
      }
      outerReceiver = await observeNextOuterReceipt({
        peer: decl.peer,
        deadlineNs,
        signal: loaded.signal,
        gateAtDelivery: () => outerGate,
        onFailure: (error) => fixedAbort.abort(error),
        record: (r) => rec.append({ type: "next-outer-delivered", ...r }),
      });
    }
    if (memoryDiagnostic) {
      const { sampleRendererMemory, inspectRendererLifecycle } =
        hooks.rendererMemoryEvidence ?? (await import("./renderer-memory-evidence.mjs"));
      if (rendererMemoryAllocatorProtocol(memoryDiagnostic.protocol)) {
        const { createMappingCollector } = await import("./renderer-memory-mappings.mjs");
        mappingCollector = (hooks.createMappingCollector ?? createMappingCollector)();
      }
      rendererMemory = createRendererMemoryStudy({
        protocol: memoryDiagnostic.protocol,
        tinyProbe: tiny?.probe,
        timing,
        reclamationArm: memoryProfile?.reclamationArm,
        append: (r) => rec.append(r),
        signal: loaded.signal,
        supervisorStartedMs: loaded.supervisorStartedMs,
        workDeadline: loaded.workDeadline,
        sample: (owned) => sampleRendererMemory(owned, buildReceipt.exe.path),
        inspect: inspectRendererLifecycle,
        captureMappings: (owned, boundary, { timeoutMs, signal }) =>
          mappingCollector.collect({
            expected: owned.handle.native,
            executablePath: buildReceipt.exe.path,
            artifactDir: labelDir,
            boundary,
            timeoutMs,
            signal,
          }),
        clock: hooks.rendererMemoryClock,
        sleep: hooks.sleep,
      });
    }
    try {
      m = fixed
        ? materializeFixed({
            fixtureDir: resolved.fixtureDir,
            sourceDir: resolved.sourceDir,
            installDir: resolved.installDir,
            workDir: join(labelDir, "work"),
            framework: profile.framework,
            adapter: profile.fixture.adapter,
            importedCss: profile.fixture.importedCss === true,
            scale: profile.fixture.scale ?? null,
          })
        : kind.materialize({ profile, resolved, workDir: join(labelDir, "work") });
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
    // A fixed zfb installs @takazudo/zfb* on purpose; its identity is the lockfile comparison.
    const local = fixed
      ? []
      : [...findLocalTakazudoPackages(m.appDir), ...findLocalTakazudoPackages(m.contentDir)];
    receipt.fixture = {
      ...(fixed ? fixedReceiptFixture(m, profile) : kind.receiptFixture(m, manifest, profile)),
      localTakazudoPackages: fixed ? null : local,
      ancestorNodeModules: ancestorNodeModules(m.appDir),
    };
    const postChecks = fixed
      ? fixedMaterializedChecks(m, resolved, verdict)
      : local.length > 0
        ? [
            {
              name: "no-local-takazudo-packages:materialized",
              ok: false,
              expected: [],
              actual: local,
            },
          ]
        : [];
    if (acceptance)
      postChecks.push({ name: "two-article-startup-fixture", ok: checkStartupArticles(m) });
    if (fixed) receipt.fixture.materializedChecks = postChecks;
    if (postChecks.some((c) => !c.ok)) {
      receipt.identity = {
        ...verdict,
        ok: false,
        checks: [...verdict.checks, ...postChecks],
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

    if (sourceTiming) {
      for (const name of Object.keys(process.env)) {
        if (
          process.env[name] &&
          (/^(?:MALLOC_|LD_)/.test(name) ||
            ["GLIBC_TUNABLES", "NODE_OPTIONS", "NODE_PATH"].includes(name))
        )
          throw Error("source timing environment contains " + name);
      }
    }
    if (fixedPeer) {
      for (const name of [
        "NODE_OPTIONS",
        "NODE_PATH",
        "LD_PRELOAD",
        "LD_AUDIT",
        "LD_LIBRARY_PATH",
        "ASTRO_DEV_BACKGROUND",
      ])
        if (process.env[name] || profile.server.env?.[name])
          throw new Error("fixed peer launch environment contains " + name);
    }
    const readinessSpec =
      fixedPeer && profile.framework === "next"
        ? {
            ...m.entry.readiness,
            require: [
              ...m.entry.readiness.require,
              String.raw`\[field-notes-dev\] external content watcher ready \(100 files; 20 ms debounce\)`,
            ],
          }
        : m.entry?.readiness;
    const fixedExecutables = fixedPeer
      ? fixedPeerExecutables(verdict.facts, profile.fixedRssSafety)
      : profile.fixedRssSafety
        ? fixedZfbExecutables(m.appDir, verdict.facts, profile.fixedRssSafety)
        : null;
    let launch = null;
    if (fixed) {
      const port =
        profile.server.port.mode === "fixed"
          ? profile.server.port.port
          : await (hooks.freePort ?? freePort)(profile.server.host);
      launch = launchCommand({
        entry: m.entry,
        appDir: m.appDir,
        launch: profile.distribution.launch ?? "direct",
        host: profile.server.host,
        port,
        nodePath: process.execPath,
        npmCli: resolveNpmCli(),
      });
      launch.port = port;
    }
    loaded.signal?.throwIfAborted();
    const peerBudget = (maximum, startedNs = hrtimeNs()) => {
      const now = hrtimeNs();
      const remaining = Math.min(
        loaded.workDeadline - Number(now / 1_000_000n),
        maximum - Math.ceil(Number(now - startedNs) / 1e6),
      );
      if (remaining <= 0) throw new Error("fixed peer work budget exhausted");
      return Math.min(maximum, remaining);
    };
    const readyTimeoutMs = rendererMemory
      ? rendererMemory.clamp("server readiness", profile.server.readyTimeoutMs)
      : fixedPeer || sourceTiming
        ? peerBudget(profile.server.readyTimeoutMs)
        : profile.server.readyTimeoutMs;
    if (nextProtocol) nextRegistryMark = markNextRegistry(resolved.registryPath);
    if (intentPath)
      intent = reserveSession(intentPath, { labelDir, registryPath: resolved.registryPath });
    if (["linux-fixed-zfb.v3", "linux-fixed-zfb.v4"].includes(profile.fixedRssSafety)) {
      fixedCustody = startFixedCustody({
        startup: profile.fixedRssSafety === "linux-fixed-zfb.v4",
        intent,
        append: (row) => rec.append(row),
        abort: (error) => fixedAbort.abort(error),
      });
      await fixedCustody.ready;
      loaded.signal?.throwIfAborted();
    }
    let sourceImage;
    if (sourceTiming) {
      const path = realpathSync(buildReceipt.exe.path);
      const fd = await open(path, "r");
      try {
        const file = await fd.stat({ bigint: true });
        const digest = createHash("sha256");
        for await (const chunk of fd.createReadStream({ autoClose: false })) {
          loaded.signal?.throwIfAborted();
          digest.update(chunk);
        }
        const after = await fd.stat({ bigint: true });
        const sha256 = digest.digest("hex");
        if (
          file.ctimeNs !== after.ctimeNs ||
          file.size !== after.size ||
          sha256 !== (hooks.sourceTimingExpectedDigest ?? CSEC_EXE_SHA256)
        )
          throw Error("source timing opened executable digest or inode contents changed");
        receipt.sourceTimingImageSha256 = sha256;
        sourceImage = { path, device: String(file.dev), inode: String(file.ino) };
      } finally {
        await fd.close();
      }
      loaded.signal?.throwIfAborted();
      receipt.sourceTimingImage = sourceImage;
    }
    spawning = true;
    startupLoop?.start(rendererMemory.signal);
    server = (hooks.startDevServer ?? startDevServer)({
      ...(fixed
        ? {
            command: launch.command,
            args: launch.args,
            readiness: createReadiness(readinessSpec, { port: launch.port }),
          }
        : { exe: buildReceipt.exe.path, extraArgs: profile.server.extraArgs ?? [] }),
      appDir: m.appDir,
      port: profile.server.port,
      host: profile.server.host,
      env: profile.server.env ?? {},
      diagnostic: profile.diagnostic === true,
      declaredZfbEnvOnly: Boolean(tiny || sourceTiming),
      readyTimeoutMs: sourceTiming ? peerBudget(profile.server.readyTimeoutMs) : readyTimeoutMs,
      logPath: join(labelDir, "server.log"),
    });
    spawning = false;
    server.handle.onNewOwned = (p) => {
      if (nextCustody && p.source === "snapshot") nextCustody.snapshot(p);
      if (shouldRegisterOwned(server.handle.native, p)) {
        try {
          appendJsonlSync(resolved.registryPath, { type: "owned", label, at: isoNow(), ...p });
        } catch (error) {
          if (!nextCustody) throw error;
          nextCustody.refuse(error);
        }
      }
    };
    if (profile.fixedRssSafety === "linux-fixed-zfb.v4")
      fixedCustody.attachStartup(server.handle, fixedExecutables);
    const abortSampling = (error) => {
      fixedAbort.abort(error);
      // Let the already-owned helper settle before signalling the server.
      // Finalization consumes the original stop promise even when browser
      // startup or a pending filesystem read outlasts the failed observation.
      const stopping = fixedSafety
        ? fixedSafety.stop()
        : Promise.resolve().then(() => fixedSafety.stop());
      void stopping.then(() => stopServer(server.handle)).catch(() => {});
    };
    if (fixedExecutables) {
      fixedSafety = startFixedRssSafety({
        nextOwner: () => nextCustody,
        server,
        fixedExecutables,
        custody: fixedCustody,
        ...(fixedPeer && {
          peer: {
            framework: profile.framework,
            rootPid: server.handle.native.pid,
            appDir: m.appDir,
            node: process.execPath,
            port: launch.port,
            image: fixedExecutables.root,
            verifyElf: fixedAstro,
          },
        }),
        limits: profile.limits,
        append: (r) => rec.append(r),
        abort: abortSampling,
      });
    }
    if (sourceTiming) {
      fixedSafety = startSourceTimingSafety({
        image: sourceImage,
        workDeadlineNs: BigInt(loaded.workDeadline) * 1000000n,
        server,
        executable: buildReceipt.exe.path,
        limits: profile.limits,
        append: (r) => rec.append(r),
        abort: abortSampling,
        ...(hooks.sourceTimingSample && { sample: hooks.sourceTimingSample }),
      });
    }
    const firstSample = rendererMemory?.startSampling(server);
    intent?.update({
      phase: "server-running",
      root: server.handle.native,
      pid: server.handle.child?.pid,
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
      ...(fixed
        ? {
            launch: {
              kind: launch.kind,
              command: launch.command,
              args: launch.args,
              port: launch.port,
            },
            readiness: readinessSpec,
          }
        : {}),
    };

    let ready;
    try {
      ready = await interruptible(server.ready, rendererMemory?.signal ?? loaded.signal);
      if (fixedPeer && ready.baseUrl !== "http://127.0.0.1:" + launch.port)
        throw new Error("fixed peer readiness URL mismatch");
      if (fixedPeer) await interruptible(fixedSafety.verifyReady(ready.readyNs), loaded.signal);
      else if (sourceTiming) await interruptible(fixedSafety.ready(), loaded.signal);
      else if (fixedSafety) await fixedSafety.verifyReady();
      browserTiming?.mark("server-readiness-observed");
    } catch (error) {
      browserTiming?.mark("server-readiness-failed");
      timeBrowserSync(browserTiming, "server-ready.append", null, { ready: false }, () =>
        rec.append({
          type: "server-ready",
          outcome: "ready-failed",
          error: String(error?.message ?? error),
          exit: server.handle.exit,
        }),
      );
      exitCode = EXIT.serverNotReady;
      summaryExtra = { stopReason: "ready-failed" };
      return exitCode;
    }
    let roles = null;
    if (fixed) {
      // Launcher tree at readiness (npm / sidecar / shim and the framework's server processes).
      const getOwned = hooks.snapshotOwned ?? (bodyPeer ? snapshotOwned : snapshotOwnedWithArgs);
      const owned = getOwned(server.handle);
      roles = bodyPeer ? [] : classifyRoles(m.entry, owned);
      const identityInputs = {
        nodeRealpath: verdict.facts?.installReceipt?.node?.realpath ?? null,
        nodeSha256: verdict.facts?.installReceipt?.node?.sha256 ?? null,
        executables: verdict.facts?.install?.executables ?? [],
        appDir: m.appDir,
        ...(hooks.processExecutable ? { exeOf: hooks.processExecutable } : {}),
        ...(hooks.processAlive ? { alive: hooks.processAlive } : {}),
      };
      if (fixedAstro) {
        verifyAstroCurrent = (phase) => {
          const owned = (hooks.snapshotOwned ?? snapshotOwnedWithArgs)(server.handle);
          const roles = fixedSafety.verifyAstroCurrent(owned, [...server.handle.known.values()]);
          rec.append({
            type: "astro-current-role",
            phase,
            checkedNs: String(hrtimeNs()),
            ...roles,
          });
        };
        verifyAstroCurrent("ready");
      }
      const checkPeer = (processes) => {
        try {
          return fixedSafety.verifyPeerProcesses(
            processes,
            [...server.handle.known.values()],
            outerDecision?.packet.worker,
          );
        } catch (error) {
          return [
            {
              name: "fixed-peer-process-identity",
              ok: false,
              method: "linux-proc-birth-role-executable",
              actual: String(error.message),
              ...(error.details && { details: error.details }),
            },
          ];
        }
      };
      const processChecks = bodyPeer
        ? checkPeer(owned)
        : processIdentityChecks({ processes: owned, roles, ...identityInputs });
      if (bodyPeer) {
        roles = processChecks
          .filter((c) => c.ok)
          .map((c) => ({
            pid: c.pid,
            ppid: c.evidence.ppid,
            comm: c.evidence.finalBirth.comm,
            role: c.role,
          }));
      }
      receipt.server.roles = roles;
      receipt.server.rootRole =
        roles.find((r) => r.pid === server.handle.native?.pid)?.role ?? null;
      const checkedKey = (p) => JSON.stringify([p.pid, p.bootId, p.startTicks ?? p.lstart]);
      const checked = new Set(owned.map(checkedKey));
      // Processes born after readiness: every one seen in the tree (or remembered by sampling)
      // is checked before the next workload edit; the first failure stops the batch there.
      checkLateProcesses = () => {
        if (lateFailure) return lateFailure;
        const now = getOwned(server.handle);
        const remembered = [...(server.handle.known?.values() ?? [])];
        const fresh = [
          ...now,
          ...remembered.filter((k) => !now.some((p) => p.pid === k.pid)),
        ].filter((p) => !checked.has(checkedKey(p)));
        if (!bodyPeer && fresh.length === 0) return null;
        for (const p of fresh) checked.add(checkedKey(p));
        const lateRoles = classifyRoles(m.entry, fresh);
        const lateChecks = bodyPeer
          ? checkPeer(now)
          : processIdentityChecks({
              processes: fresh,
              roles: lateRoles,
              ...identityInputs,
            });
        receipt.server.processIdentityLate = [
          ...(receipt.server.processIdentityLate ?? []),
          ...lateChecks,
        ];
        const bad = lateChecks.filter((c) => !c.ok);
        rec.append({
          type: "identity",
          ok: bad.length === 0,
          ...(bad.length ? { outcome: "identity-rejected" } : {}),
          phase: "before-edit",
          checks: lateChecks,
        });
        if (bad.length)
          lateFailure = { reason: "process-identity", outcome: "identity-rejected", checks: bad };
        return lateFailure;
      };
      receipt.server.processIdentity = processChecks;
      if (processChecks.some((c) => !c.ok)) {
        // A process of the tree runs an executable the install did not record (e.g. PATH's node):
        // refused before the browser opens and before any workload edit.
        receipt.identity = {
          ...receipt.identity,
          ok: false,
          checks: [...receipt.identity.checks, ...processChecks],
        };
        rec.append({
          type: "identity",
          ok: false,
          outcome: "identity-rejected",
          phase: "server-ready",
          checks: processChecks,
        });
        summaryExtra = { stopReason: "identity-rejected", identityRejected: true };
        exitCode = EXIT.identityRejected;
        return exitCode;
      }
    }
    timeBrowserSync(browserTiming, "server-ready.append", null, { ready: true }, () =>
      rec.append({
        type: "server-ready",
        baseUrl: ready.baseUrl,
        readyMs: nsToMs(ready.readyNs - server.startedNs),
        native: server.handle.native,
        launcher: fixed ? (receipt.server.rootRole ?? "unclassified") : "none",
        ...(fixed ? { roles } : {}),
      }),
    );
    receipt.server.baseUrl = ready.baseUrl;
    receipt.server.readyMs = nsToMs(ready.readyNs - server.startedNs);

    if (profile.startupAcceptance || tiny?.probe === "A") {
      Object.assign(
        receipt[acceptanceKey],
        await verifyStartup({
          server,
          ready,
          firstSample,
          records: rec.records,
          study: rendererMemory,
          fetchImpl: hooks.fetch,
        }),
      );
      if (tiny) await rendererMemory.passiveIdle();
      return exitCode;
    }
    if (browserTiming)
      Object.assign(
        receipt[acceptanceKey],
        await browserTiming.asyncSpan("initial-sample.verify", null, () =>
          verifyStartupSample({
            server,
            ready,
            firstSample,
            records: rec.records,
            study: rendererMemory,
          }),
        ),
      );
    const browserSignal = rendererMemory?.signal ?? loaded.signal;
    browserSignal?.throwIfAborted();
    const launchTimeoutMs = rendererMemory
      ? () => rendererMemory.clamp("browser startup", 30_000)
      : fixedPeer || sourceTiming
        ? () => peerBudget(30000)
        : undefined;
    const browserOptions = {
      ...profile.browser,
      ...(fixedPeer
        ? {
            diagnosticExecutable: {
              path: profile.fixedPeerBrowser.executablePath,
              sha256: profile.fixedPeerBrowser.executableSha256,
              timeoutMs: launchTimeoutMs(),
            },
          }
        : {}),
      ...(memoryProfile || tiny
        ? {
            diagnosticExecutable: {
              path: memoryDiagnostic.browserExecutable,
              sha256: memoryDiagnostic.browserExecutableSha256,
              // Check admission before any browser setup. Recompute at launch.
              timeoutMs: launchTimeoutMs(),
            },
          }
        : {}),
    };
    const launchBrowser = hooks.launchOwnedBrowser ?? launchOwnedBrowser;
    browserHandle = await timeBrowserAsync(browserTiming, "browser.initialize", null, (parentId) =>
      launchBrowser(browserOptions, {
        timing: browserTiming,
        parentId,
        signal: browserSignal,
        ...(intent && {
          onReadState(details, state) {
            // The independent supervisor copies this intent even if a pending
            // read never settles and the controller cannot reach its finalizer.
            const read = { ...details, state };
            try {
              timeBrowserSync(browserTiming, "browser.file.ownership", parentId, read, () =>
                intent.update({ browserExecutableRead: read }),
              );
            } catch (error) {
              if (state === "pending" || state === "fulfilled") throw error;
              stderr(`cannot record failed browser read in admission hold: ${error.message}`);
            }
          },
        }),
        launchTimeoutMs,
      }),
    );
    browserTiming?.mark("browser-initialization-return-observed");
    browserSignal?.throwIfAborted();
    if (fixedPeer) {
      const imageErrors = fixedPeerBrowserImageErrors(
        browserHandle.identity,
        profile.fixedPeerBrowser,
      );
      if (imageErrors.length) {
        receipt.browser = browserHandle.identity;
        throw new Error("fixed peer browser image refused: " + imageErrors.join("; "));
      }
    }
    const observer = browserHandle.observer;
    observer.baseUrl = ready.baseUrl;
    if (nextProtocol) {
      const cfg = {
        framework: profile.framework,
        rootPid: server.handle.native.pid,
        appDir: m.appDir,
        node: process.execPath,
        port: launch.port,
        image: fixedExecutables.root,
      };
      const { chain, socket } = readPeerRoles(fixedSafety.qualifiedPeer.chain, cfg, true, true);
      const contract = buildNextBootstrapContract({
        ...cfg,
        protocol: NEXT_BOOTSTRAP_PROTOCOL,
        chain,
        socket,
        bootstrapSha256: "463a33ccdf45500486481c697ca9b7445fae5998410b82d3d5485c0b85b770eb",
      });
      // The holder constructor now owns fd4 through spawn or failure.
      if (outerProtocol) holderSocketOwned = false;
      nextCustody = startNextPeerCustody({
        prewriteDecision: decisionProtocol || outerProtocol,
        ...(outerProtocol && {
          outerOffer: { fd: 4, nonce: loaded.nextOuter.nonce },
          onOuterAuthority: (header) => {
            const authority = {
              ...header,
              outer: loaded.nextOuter.outer,
              credentials: loaded.nextOuter.peer,
              startNs: String(BigInt(loaded.supervisorStartedMs) * 1000000n),
              deadlineNs: String(BigInt(loaded.workDeadline) * 1000000n),
            };
            outerGate = outerTerminalGate({ authority });
            outerReceiver.sendAuthority(header);
            rec.append({
              type: "next-outer-authority",
              authority,
              nonceSha256: nextOuterNonceDigest(header.nonce),
            });
          },
        }),
        contract,
        intent,
        append: (r) => rec.append(r),
        abort: (e) => {
          fixedAbort.abort(e);
          void stopServer(server.handle).catch(() => {});
        },
      });
      await interruptible(nextCustody.ready, loaded.signal);
      if (outerProtocol) {
        const direct = nextOuterChildren(process.pid).map((pid) => linuxBirth(pid));
        const matches = direct.filter(
          (p) =>
            p.ppid === process.pid &&
            readlinkSync("/proc/" + p.pid + "/exe") ===
              browserHandle.identity.launchedExecutablePath,
        );
        assert.equal(matches.length, 1, "unique launched browser child required");
        outerWiring = {
          outer: loaded.nextOuter.outer,
          supervisor: linuxBirth(process.ppid),
          controller: linuxBirth(process.pid),
          lock: loaded.outerLock,
          server: server.handle.native,
          browser: matches[0],
          holder: nextCustody.identity.birth,
          browserPath: browserHandle.identity.launchedExecutablePath,
        };
      }
      const prepare = observer.prepare.bind(observer);
      let first = true;
      observer.prepare = async (route, ...args) => {
        if (!first) return prepare(route, ...args);
        first = false;
        await nextCustody.openWindow("first-route", peerBudget(30000));
        let value;
        try {
          value = await prepare(route, ...args);
        } catch (firstFailure) {
          try {
            await nextCustody.closeWindow("first-route");
          } catch (error) {
            try {
              rec.append({
                type: "next-custody-secondary-close-failure",
                error: String(error),
                firstFailure: String(firstFailure),
              });
            } catch (e) {
              stderr(`Next custody closure evidence write failed: ${e.message}`);
            }
          }
          throw firstFailure;
        }
        await nextCustody.closeWindow("first-route");
        return value;
      };
    }
    if (fixedPeer) observer.peerSocketTracking = new Map();
    if (fixedAstro) observer.fixedAstroObservation = true;
    // Fixed distributions declare their own HMR sockets; zfb livereload needs none.
    if (fixed) observer.updateSockets = m.entry.updateSignal?.sockets ?? [];
    observer.onStray = (payload) =>
      rec.append({ type: "late-token", label: "not-success", armedToken: null, payload });
    receipt.browser = browserHandle.identity;
    timeBrowserSync(browserTiming, "browser-ready.append", null, {}, () =>
      rec.append({ type: "browser-ready", browser: browserHandle.identity }),
    );
    startupLoop?.browserReady();
    writeJsonSync(receiptPath, receipt);

    const sampleServer = () =>
      timed(timing, "run.sampleServer", {}, () => {
        if (sourceTiming) return fixedSafety.latest();
        const raw = server.sampleResources(timing);
        const s = fixedSafety ? fixedContextSample(raw) : raw;
        samples.push(s);
        return s;
      });

    const fixtureOps = kind.ops(m, profile);

    if (batch) {
      if (rendererMemory) await rendererMemory.beforeEdits();
      startupLoop?.stop("before-edits");
      if (profile.resourceStudy) {
        study = createResourceStudy({
          study: profile.resourceStudy,
          signal: loaded.signal,
          sampleNow: () => server.sampleResourcesAsync({ withThreads: true }),
          append: (r) => rec.append(r),
          clock: { now: hrtimeNs },
          sleep: hooks.sleep,
        });
        await study.start();
      }
      const recorder = study
        ? {
            append: (r) => {
              study.onRecord(r);
              return rec.append(r);
            },
          }
        : rec;
      if (sourceTiming) {
        const decided = await fixedSafety.decide({
          site: "pre-batch",
          capNs: BigInt(loaded.workDeadline) * 1_000_000n,
          signal: loaded.signal,
        });
        rec.append({ type: "source-timing-decision", ...decided.decision });
      }
      const { accounting, stopped } = await runBatch({
        scenarios: runScenarios,
        deps: {
          recorder,
          signal: rendererMemory?.signal ?? loaded.signal,
          rendererMemory,
          safetySignal: fixedAbort?.signal,
          ...(sourceTiming && { sourceTimingSafety: fixedSafety }),
          ...((fixedPeer || sourceTiming) && {
            stageBudget: peerBudget,
            workDeadlineNs: BigInt(loaded.workDeadline) * 1_000_000n,
          }),
          ...(nextProtocol && {
            ...(outerProtocol && {
              ...nextOuterBatchHooks({
                safety: fixedSafety,
                gate: () => outerGate,
                readProductGraph: () => readNextOuterProductGraph(server.handle.native),
                verifyWiring: () => verifyNextOuterWiring(outerWiring),
                operation: profile.fixedNextOuterObserver.operation,
                workDeadlineNs: BigInt(loaded.workDeadline) * 1000000n,
                signal: loaded.signal,
                append: (r) => rec.append(r),
                onDecision: (decision, cadence) => {
                  outerDecision = decision;
                  outerCadence = cadence;
                },
                onRefusal: (stage) => {
                  outerRefusals[stage] = (outerRefusals[stage] ?? 0) + 1;
                },
              }),
            }),
            ...(decisionProtocol && {
              checkPeerWriteTime: (stamp) => nextCustody.checkWriteTime(stamp),
              beforePeerDecision: async ({ attemptStart, attemptMs }) => {
                const workDeadlineNs = BigInt(loaded.workDeadline) * 1000000n;
                const attemptDeadlineNs = attemptStart + BigInt(attemptMs) * 1000000n;
                await fixedSafety.verifyPrewriteTerminal({
                  getOwned: () => snapshotOwned(server.handle),
                  signal: loaded.signal,
                  workDeadlineNs:
                    attemptDeadlineNs < workDeadlineNs ? attemptDeadlineNs : workDeadlineNs,
                });
              },
            }),
            beforePeerMutation: () => {
              const reason = checkLateProcesses();
              rec.append({
                type: "next-custody-pre-mutation",
                ...(decisionProtocol && {
                  decisionProof: nextCustody.decisionProof,
                  graphMatches: !reason,
                  status: "provisional-census",
                }),
                reason,
                checkedNs: String(process.hrtime.bigint()),
              });
              if (reason)
                throw new Error("Next pre-mutation identity refused: " + JSON.stringify(reason));
            },
          }),
          afterScenario: study?.afterScenario,
          beforeCleanup: rendererMemory
            ? () => rendererMemory.enter("restoration")
            : study?.beforeCleanup,
          haltReason:
            rendererMemory || study || checkLateProcesses
              ? () =>
                  rendererMemory?.failure ?? study?.haltReason() ?? checkLateProcesses?.() ?? null
              : undefined,
          clock: { now: hrtimeNs },
          server: {
            ...server,
            sampleResources: sampleServer,
            ...(fixedSafety && {
              sampleResourcesAsync: async (...args) =>
                fixedContextSample(await server.sampleResourcesAsync(...args)),
            }),
          },
          page: observer,
          fs: fixtureOps,
          fixedAstroFooter: profile.fixedRssSafety === FIXED_ASTRO_FOOTER_PROTOCOL,
          fixedAstroCategory: profile.fixedRssSafety === FIXED_ASTRO_CATEGORY_PROTOCOL,
          fixedAstroTitle: profile.fixedRssSafety === FIXED_ASTRO_TITLE_PROTOCOL,
          beforeAstroWrite: verifyAstroCurrent,
          fixedPeer: bodyPeer,
          verifyBodyRevision:
            bodyPeer || tiny?.probe === "B" || (!fixed && profile.fixture.kind === "field-notes"),
          requiredBodyBaseline: tiny?.probe === "B" ? "Body marker body-a-v0 for article a." : null,
          limits: profile.limits,
          timeouts: profile.timeouts,
          startedNs,
          accountingRef,
          provenance:
            profile.diagnostic === true && !rendererMemory
              ? {
                  traceEnabled: traceEnabledIn(server.zfbEnv),
                  renderMode: resolveLazyDevRender(server.zfbEnv),
                  pageFor: fixtureOps.pageFor ?? null,
                }
              : null,
        },
      });
      if (rendererMemory) {
        summaryExtra.accountingFinal = accounting;
        await rendererMemory.finalIdle({
          expectedRestorationPath: tiny?.probe === "B" ? m.articles.a : null,
          stopped,
          stopReason: accounting.stopReason,
          cleanup: rec.records.findLast((r) => r.type === "cleanup"),
        });
      }
      let studyFailure = null;
      if (study) {
        await study.finalIdle({ stopped, stopReason: accounting.stopReason });
        await study.stop();
        studyFailure = study.failure;
        study = null;
      }
      summaryExtra = { accountingFinal: accounting, resourceStudyFailure: studyFailure };
      exitCode = batchExitCode({
        lateFailure,
        studyFailure,
        rendererFailure: rendererMemory?.failure,
        accounting,
        stopped,
        fixedPeer,
      });
      if (lateFailure) {
        summaryExtra.identityRejected = true;
        receipt.identity = {
          ...receipt.identity,
          ok: false,
          checks: [...receipt.identity.checks, ...lateFailure.checks],
        };
      }
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
    browserHandle ??= error?.partialBrowser;
    rec.append({
      type: "controller-error",
      browserCleanupError: error?.browserCleanupError,
      outcome: "controller-error",
      error: String(error?.stack ?? error),
    });
    exitCode = EXIT.controllerError;
    summaryExtra = { ...summaryExtra, stopReason: summaryExtra.stopReason ?? "controller-error" };
    return exitCode;
  } finally {
    startupLoop?.stop("finalizing");
    loaded.onFinalizing?.();
    // Best-effort from here on: a failing record write must never skip the server stop or the summary.
    let evidenceFailed = false;
    const safeAppend = (r) => {
      try {
        rec.append(r);
      } catch (error) {
        evidenceFailed = true;
        stderr(`record write failed (${r.type}): ${error?.message ?? error}`);
      }
    };
    const terminalNow = performance.now();
    const terminalEnd =
      terminalNow +
      Math.min(
        60000,
        rendererMemory || fixedPeer
          ? loaded.supervisorDeadlineMs - Number(hrtimeNs() / 1_000_000n)
          : 60000,
      );
    const bounded = async (phase, ms, operation) => {
      const remaining = Math.max(1, Math.min(ms, terminalEnd - performance.now()));
      let timer;
      try {
        return await Promise.race([
          operation(),
          new Promise((_, reject) => {
            timer = setTimeout(
              () =>
                reject(
                  Object.assign(new Error(`${phase} exceeded its budget`), {
                    code: "FINALIZATION_TIMEOUT",
                  }),
                ),
              remaining,
            );
          }),
        ]);
      } finally {
        clearTimeout(timer);
      }
    };
    const unresolved = [];
    if (fixedSafety) {
      try {
        const failure = await bounded("fixed-rss-stop", 1000, () => fixedSafety.stop());
        if (failure) {
          if (sourceTiming) receipt.sourceTimingFailure = failure;
          else receipt.fixedRssFailure = failure;
          evidenceFailed = true;
          exitCode = EXIT.controllerError;
        }
        if (fixedSafety.unresolvedHelper)
          unresolved.push({
            resource: sourceTiming ? "source-timing-sampler" : "fixed-rss-helper",
            helper: fixedSafety.unresolvedHelper,
          });
      } catch (error) {
        unresolved.push({
          resource: sourceTiming ? "source-timing-sampler" : "fixed-rss-sampler",
          error: String(error),
          ...(sourceTiming &&
            fixedSafety.unresolvedHelper && { pendingAtTimeout: fixedSafety.unresolvedHelper }),
        });
      }
    }
    if (fixedCustody) {
      try {
        await bounded("fixed-custody-stop", 500, () => fixedCustody.stop());
      } catch (error) {
        unresolved.push({
          resource: "fixed-descriptor-custody",
          ...fixedCustody.identity,
          error: String(error),
        });
        evidenceFailed = true;
        exitCode = EXIT.controllerError;
      }
    }
    if (outerProtocol) {
      outerGate?.retire();
      try {
        if (holderSocketOwned) closeSync(4);
        if (outerReceiver) await bounded("next-outer-stop", 500, () => outerReceiver.stop());
      } catch (error) {
        unresolved.push({ resource: "next-outer-channel", error: String(error) });
        evidenceFailed = true;
      }
    }
    if (nextCustody) {
      try {
        await bounded("next-custody-stop", 500, () => nextCustody.stop());
      } catch (e) {
        unresolved.push({ resource: "next-custody", ...nextCustody.identity, error: String(e) });
        evidenceFailed = true;
      }
    }
    if (rendererMemory)
      try {
        // Stop launching samples, finish the bounded helper and retain its discoveries
        // while the product is still alive. Only then may teardown signal the server.
        await bounded("renderer-memory-sampler-stop", 1000, () => rendererMemory.beginShutdown());
        if (rendererMemory.unresolvedHelper)
          unresolved.push({
            resource: "renderer-memory-sampler",
            helper: rendererMemory.unresolvedHelper,
          });
      } catch (error) {
        evidenceFailed = true;
        unresolved.push({ resource: "renderer-memory-sampler", error: String(error) });
        stderr(`renderer-memory sampler stop failed: ${error?.message ?? error}`);
      }
    if (study) {
      try {
        await bounded("sampler-stop", 3000, () => study.stop());
      } catch (error) {
        evidenceFailed = true;
        stderr(`resource-study sampler stop failed: ${error?.message ?? error}`);
      }
    }
    let stopReport = {
      absent: !spawning,
      reason: spawning ? "spawn-interrupted" : "no-server-started",
    };
    if (server) {
      try {
        stopReport = await bounded("server-stop", 25000, () => stopServer(server.handle));
      } catch (error) {
        stopCancellation.abort();
        stopReport = {
          absent: false,
          error: String(error?.message ?? error),
          code: error?.code ?? null,
          phase: "server-stop",
          expected: error.expected ?? null,
          actual: error.actual ?? null,
          differingFields: error.differingFields ?? [],
          steps: error.steps ?? [],
          remaining: error.remaining ?? [...(server.handle.known?.values() ?? [])],
          exit: server.handle.exit,
        };
      }
      try {
        logClose = await bounded("log-close", 4000, () => server.closeLog());
      } catch (error) {
        unresolved.push({ resource: "server.log", error: String(error) });
        logClose = { error: String(error?.message ?? error) };
      }
      if (logClose?.pipes === "timeout") {
        unresolved.push({ resource: "server-pipes", reason: "open after drain timeout" });
        stopReport = { ...stopReport, absent: false, unresolved };
      }
      if (logClose?.errors?.length) evidenceFailed = true;
      if (fixedAstro) {
        const valid = verifyAstroPopulation(server.handle.native, stopReport);
        safeAppend({ type: "astro-terminal-population", valid, resourceAbsent: stopReport.absent });
        if (!valid) evidenceFailed = true;
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
        evidenceFailed = true;
        stderr(`registry write failed: ${error?.message ?? error}`);
      }
    }
    if (nextProtocol && server) {
      try {
        receipt.nextPeerPopulation = reconcileNextPopulation({
          chain: fixedSafety?.qualifiedPeer?.chain,
          worker: nextCustody?.qualified,
          terminalProof:
            nextCustody?.terminalProof ??
            (outerDecision && {
              birth: outerDecision.packet.worker,
              pidfdFlags: outerDecision.packet.pidfdFlags,
              pollMask: outerDecision.packet.pollMask,
              observedNs: outerDecision.packet.terminalNs,
              source: "independent-outer",
            }),
          stop: stopReport,
          registry: readNextRegistry(resolved.registryPath, nextRegistryMark, label),
        });
      } catch (error) {
        receipt.nextPeerPopulation = {
          ok: false,
          resourceAbsent: stopReport.absent,
          error: String(error),
        };
      }
      safeAppend({ type: "next-peer-population", ...receipt.nextPeerPopulation });
      if (!receipt.nextPeerPopulation.ok) evidenceFailed = true;
    }
    if (browserHandle) {
      try {
        for (const e of await bounded("browser-close", 21000, () =>
          (hooks.closeOwnedBrowser ?? closeOwnedBrowser)(browserHandle, {
            timeoutMs: BROWSER_CLOSE_TIMEOUT_MS,
          }),
        )) {
          unresolved.push({ resource: "browser", ...e });
          safeAppend({
            type: "controller-error",
            phase: e.step,
            error: e.error,
            timedOut: e.timedOut ?? false,
          });
        }
      } catch (error) {
        unresolved.push({ resource: "browser", error: String(error) });
        safeAppend({
          type: "controller-error",
          phase: "browser-close",
          error: String(error?.message ?? error),
        });
      }
    }
    if (mappingCollector) {
      try {
        await bounded("mapping-collector-stop", 3000, () => mappingCollector.stop());
      } catch (error) {
        mappingCollector.releaseUnresolvedHandle();
        unresolved.push({
          resource: "mapping-collector",
          pid: mappingCollector.activePid,
          boundary: mappingCollector.activeBoundary,
          collectorReceipt: join("mappings", `${mappingCollector.activeBoundary}.collector.json`),
          error: String(error),
        });
        safeAppend({
          type: "controller-error",
          phase: "mapping-collector-stop",
          error: String(error),
        });
      }
    }
    if (rendererMemory) {
      rendererMemory.finishSampling();
      // The original sampler outcome survives readiness or trace-finalization failure.
      if (memoryProfile) receipt.rendererMemoryStudy.samplerFailure = rendererMemory.samplerFailure;
      if (acceptance) receipt[acceptanceKey].samplerFailure = rendererMemory.samplerFailure;
      const accounting = summaryExtra.accountingFinal;
      const completedSchedule =
        rendererMemory.idleMs !== null &&
        accounting?.attempted === RENDERER_MEMORY_LIMITS.edits &&
        accounting.success === accounting.attempted &&
        accounting.stopReason === null;
      try {
        memoryReport = rendererMemory.finish({
          logClose,
          completedSchedule,
          cleanup: rec.records.findLast((r) => r.type === "cleanup"),
          serverAbsent: stopReport.absent && !stopReport.error && unresolved.length === 0,
        });
        if (acceptance)
          Object.assign(receipt[acceptanceKey], {
            ...(tiny && {
              controllerTiming: "controller-timing.json",
              idleMs: memoryReport.idleMs,
              sampleInvocations: memoryReport.sampleInvocations,
              helperStarts: memoryReport.helperStarts,
              helperLaunchAttempts: memoryReport.helperLaunchAttempts,
              restoration: memoryReport.restoration,
              scheduleComplete:
                tiny.probe === "B" ? completedSchedule : memoryReport.idleMs >= 30_000,
            }),
            samples: memoryReport.samples,
            bodyEdits: rec.records.filter((r) => r.type === "attempt" && r.op === "edit-body")
              .length,
            // A complete native trace with the observation-only startup anchor excludes actions.
            reclamationActions: tiny
              ? memoryReport.lifecycle?.performedActionCompletions ||
                (memoryReport.lifecycle?.complete &&
                memoryReport.lifecycle.reclamationActionStarts === 0
                  ? 0
                  : null)
              : memoryReport.lifecycle?.complete === true
                ? 0
                : null,
            peakSummedRssKiB: memoryReport.peakSummedRssKiB,
            failure: memoryReport.failure,
            lifecycleComplete: memoryReport.lifecycle?.complete === true,
            logComplete: memoryReport.logComplete,
            stop: stopReport,
          });
        if (
          tiny &&
          (!receipt[acceptanceKey].scheduleComplete ||
            receipt[acceptanceKey].reclamationActions !== 0 ||
            timing.report.dropped)
        )
          evidenceFailed = true;
        safeAppend({ type: "renderer-memory-evidence", ...memoryReport });
      } catch (error) {
        evidenceFailed = true;
        stderr(`renderer-memory evidence verification failed: ${error?.message ?? error}`);
      }
      if (exitCode === EXIT.ok) {
        if (!memoryReport?.observationComplete) exitCode = EXIT.controllerError;
        else if (memoryReport.failure) exitCode = EXIT.stopped;
      }
    }
    recordSamplerClosureAudits(unresolved, fixedSafety, rendererMemory, safeAppend);
    let stopFailure = null;
    if (!stopReport.absent || stopReport.error || unresolved.length) {
      stopFailure = {
        absent: Boolean(stopReport.absent),
        error: stopReport.error ?? null,
        code: stopReport.code ?? null,
        exitBefore: exitCode,
      };
      exitCode = EXIT.ownedServerAlive;
    }
    if (intent) {
      if (stopFailure) {
        try {
          intent.update({
            phase: "cleanup-unresolved",
            stopReport,
            unresolved,
          });
        } catch (error) {
          stderr(`cannot update admission hold ${intentPath}: ${error.message}`);
        }
        const child = server?.handle.child;
        child?.stdout?.destroy();
        child?.stderr?.destroy();
        child?.unref();
      } else {
        intent.release();
      }
    }
    try {
      writeBrowserStartupTiming();
    } catch (error) {
      evidenceFailed = true;
      stderr(`browser startup timing write failed: ${error?.message ?? error}`);
    }
    if (evidenceFailed && exitCode === EXIT.ok) exitCode = EXIT.controllerError;
    if (acceptance) {
      receipt[acceptanceKey].complete =
        exitCode === EXIT.ok &&
        (tiny?.probe === "B" || Boolean(receipt[acceptanceKey].live?.markerMatched));
      receipt[acceptanceKey].unresolvedResources = unresolved;
    }
    try {
      writeJsonSync(receiptPath, receipt);
    } catch (error) {
      evidenceFailed = true;
      stderr(`receipt write failed: ${error?.message ?? error}`);
    }
    if (evidenceFailed && exitCode === EXIT.ok) exitCode = EXIT.controllerError;
    safeAppend({ type: "run-end", exitCode });
    if (evidenceFailed && exitCode === EXIT.ok) exitCode = EXIT.controllerError;
    const live = summaryExtra.accountingFinal ?? accountingRef.current;
    const accounting = live ? { ...live } : { ...newAccounting(planned) };
    if (!summaryExtra.accountingFinal && summaryExtra.stopReason) {
      accounting.stopReason = accounting.stopReason ?? summaryExtra.stopReason;
      accounting.error += 1;
    }
    try {
      if (timing) writeJsonSync(join(labelDir, "controller-timing.json"), timing.report);
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
        ...(acceptance && { diagnostic: true, [acceptanceKey]: receipt[acceptanceKey] }),
        ...(memoryProfile
          ? {
              diagnostic: true,
              rendererMemoryStudy: memoryReport ?? {
                protocol: memoryProfile.protocol,
                reclamationArm: memoryProfile.reclamationArm,
                scheduleComplete: false,
                observationComplete: false,
              },
            }
          : {}),
        resourceSamples:
          sourceTiming && fixedSafety
            ? {
                count: fixedSafety.report().accepted,
                peak:
                  fixedSafety.report().peakSummedOwnedRssKiB === null
                    ? null
                    : {
                        summedRssKiB: fixedSafety.report().peakSummedOwnedRssKiB,
                        label:
                          "observed complete per-birth owned RSS; separate from partial observations",
                      },
              }
            : {
                count: samples.length,
                peak:
                  fixedSafety && !samples.some((s) => typeof s.summedRssKiB === "number")
                    ? null
                    : sampledPeak(samples),
              },
        ...(fixedSafety &&
          (sourceTiming
            ? { sourceTimingSafety: fixedSafety.report() }
            : { fixedRssSafety: fixedSafety.report() })),
        resourceStudy:
          command === "run" && profile.resourceStudy
            ? {
                ...summarizeResourceStudy(rec.records, profile.resourceStudy),
                failure: summaryExtra.resourceStudyFailure ?? null,
              }
            : null,
        ownedServerAbsentAfterStop: Boolean(stopReport.absent),
        stopFailure,
        serverExit: server?.handle.exit ?? null,
        wallMs: nsToMs(hrtimeNs() - startedNs),
      };
      writeJsonSync(summaryPath, summary);
      if (outerProtocol) {
        writeJsonSync(join(labelDir, "next-outer-outcome.json"), {
          label,
          nonce: loaded.nextOuter.nonce,
          providerPid: loaded.nextOuter.outer.pid,
          controllerPid: process.pid,
          ...nextOuterRunOutcome({
            stopReport,
            stopFailure,
            unresolved,
            evidenceFailed,
            population: receipt.nextPeerPopulation,
            safetyFailure: fixedSafety?.report().failure,
            operation: profile.fixedNextOuterObserver.operation,
            records: rec.records,
            exitCode,
            accounting,
            outerDecision,
            refusals: outerRefusals,
          }),
        });
      }
    } catch (error) {
      evidenceFailed = true;
      stderr(`summary write failed: ${error?.message ?? error}`);
    }
    if (evidenceFailed && !stopFailure) return exitCode || EXIT.controllerError; // eslint-disable-line no-unsafe-finally
    if (stopFailure) {
      stderr(
        "owned server processes were not verified absent after stop; the next batch will refuse to start",
      );
      // Returning from finally deliberately overrides the try block's exit code.
      return exitCode; // eslint-disable-line no-unsafe-finally
    }
    if (memoryDiagnostic || fixedAbort) return exitCode; // eslint-disable-line no-unsafe-finally
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

/** Receipt fields only a fixed-distribution run has. */
function fixedReceiptFixture(m, profile) {
  return {
    name: m.manifest.name,
    kind: "field-notes",
    scale: m.scale,
    scaleId: m.scaleId,
    adapter: profile.fixture.adapter,
    adapterSha256: m.adapterSha256,
    sourceDir: profile.fixture.source.dir,
    sourcePins: profile.fixture.source,
    placement: m.placement,
    layout: m.layout,
    trailingSlash: true,
    materializedDatasetSha256: m.materializedDatasetSha256,
    configFile: m.configFile,
    configSha256: m.configSha256,
    configProvenance: m.configProvenance,
    configBaseSha256: m.configBaseSha256,
    workDir: m.workDir,
    appDir: m.appDir,
    contentDir: m.contentDir,
    managedCss: { declaredAddition: false },
    selectors: Object.fromEntries(
      Object.entries(FN_SEL).map(([k, v]) => [k, typeof v === "function" ? v("{slug}") : v]),
    ),
    mode: "fixed-distribution",
    framework: m.framework,
    app: m.entry.app,
    pinned: m.entry.pinned,
    appTreeSha256: m.appTreeSha256,
    helperSha256: m.helperSha256,
    helperProvenance: m.helperProvenance,
    importedCss: m.importedCss,
    adapterFiles: m.entry.adapterFiles,
    nodeModules: m.nodeModules,
    distributionsSha256: m.entry.manifestSha256,
  };
}

/**
 * After materialization: the copied app (imported-CSS targets aside) equals the install's app,
 * and the copied node_modules equals the install's recorded tree. A difference refuses the run
 * before the server starts.
 */
function fixedMaterializedChecks(m, resolved, verdict) {
  const exclude = [...(m.entry.appTree.exclude ?? []), ...m.importedCss.map((f) => f.target)];
  const ex = { ...m.entry, appTree: { ...m.entry.appTree, exclude } };
  const installApp = join(resolved.installDir, m.entry.app);
  const installTree = appTreeHash(installApp, ex).sha256;
  const recordedNm = verdict.facts?.install?.nodeModules?.sha256 ?? null;
  return [
    {
      name: "materialized-app-equals-install",
      ok: m.appTreeSha256 === installTree,
      expected: installTree,
      actual: m.appTreeSha256,
    },
    {
      name: "materialized-node-modules-equals-install",
      ok: recordedNm !== null && m.nodeModules.sha256 === recordedNm,
      expected: recordedNm,
      actual: m.nodeModules.sha256,
    },
  ];
}
