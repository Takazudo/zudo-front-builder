import { fork } from "node:child_process";
import { readNextOuterDeclaration, takeNextOuterChannel } from "./next-outer-channel.mjs";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { admissionPath } from "./admission.mjs";
import { readJson, writeJsonSync } from "./util.mjs";
import { LABEL_RE, loadProfile } from "./profile.mjs";
import { EXIT, findGitWorkTree } from "./run.mjs";

export const FINALIZATION_MS = 60_000;

const workerFile = new URL("./session-worker.mjs", import.meta.url);
const monotonicMs = () => Number(process.hrtime.bigint() / 1_000_000n);

// This timer lives outside the controller. Synchronous preparation, a stalled
// teardown or a referenced pipe inside the controller cannot delay its firing.
export async function superviseSession({
  profilePath,
  label,
  command,
  scenarios = null,
  stderr = (s) => process.stderr.write(s + "\n"),
}) {
  const started = monotonicMs();
  const { profile, resolved } = loadProfile(profilePath);
  const startup = Boolean(profile.startupAcceptance);
  const tiny = Boolean(profile.tinyAcceptance);
  const diagnostic = Boolean(profile.rendererMemoryStudy || startup || tiny);
  const bounded = diagnostic || Boolean(profile.fixedRssSafety);
  if ((command === "startup-acceptance") !== startup || (command === "tiny-acceptance") !== tiny) {
    stderr("acceptance requires its own profile and command");
    return EXIT.usage;
  }
  if (
    bounded &&
    (command !== (tiny ? "tiny-acceptance" : startup ? "startup-acceptance" : "run") ||
      scenarios !== null ||
      process.platform !== "linux")
  ) {
    stderr("Linux safety observations require a standalone Linux run without --scenario");
    return EXIT.usage;
  }
  const deadline = started + profile.limits.wallCeilingMs;
  const supervisorId = randomUUID();
  const labelDir = join(resolved.outDir, String(label));
  const recordFailure = (failure) => {
    if (!LABEL_RE.test(label) || findGitWorkTree(resolved.outDir)) return;
    const holdPath = process.platform === "linux" ? admissionPath() : null;
    let intent = null;
    let intentReadError = null;
    if (holdPath && existsSync(holdPath)) {
      try {
        const active = readJson(holdPath);
        if (active.controllerPid === failure.controllerPid && active.labelDir === labelDir)
          intent = active;
      } catch (error) {
        intentReadError = String(error.message);
      }
    }
    mkdirSync(resolved.outDir, { recursive: true });
    const report = join(resolved.outDir, label + ".supervisor-" + supervisorId + ".json");
    writeJsonSync(report, {
      schema: "dev-browser-observe.supervisor-failure.v1",
      supervisorId,
      labelDir,
      profilePath,
      ...failure,
      ownedServerAbsentAfterStop: false,
      unresolvedResources: intent?.unresolved ?? ["session cleanup requires verification"],
      registryPath: resolved.registryPath,
      holdPath,
      intent,
      intentReadError,
    });
    stderr("session supervisor failure record: " + report);
  };
  const outer = profile.fixedNextOuterObserver
    ? readNextOuterDeclaration(profile.fixedNextOuterObserver)
    : null;
  let controllerExit = null;
  const code = await superviseController({
    worker: workerFile,
    ...(outer && { nextOuterChannel: outer }),
    args: {
      profilePath,
      label,
      command,
      scenarios,
      supervisorId,
      ...(outer && {
        nextOuter: {
          outer: outer.outer,
          peer: outer.peer,
          nonce: outer.nonce,
          sockets: outer.sockets,
        },
      }),
      ...(bounded ? { supervisorStartedMs: started, supervisorDeadlineMs: deadline } : {}),
    },
    deadline,
    diagnostic: bounded,
    onVerifiedExit: (result) => {
      controllerExit = result;
    },
    stderr,
    recordFailure,
  });
  if (code === EXIT.ok || (bounded && controllerExit && existsSync(labelDir))) {
    // Only the supervisor can attest that the controller actually exited.
    writeJsonSync(join(labelDir, "supervisor-completion.json"), {
      schema: "dev-browser-observe.supervisor-completion.v1",
      supervisorId,
      exitCode: code,
      at: new Date().toISOString(),
      ...(bounded
        ? {
            diagnostic,
            ...(profile.fixedRssSafety && { fixedRssSafety: profile.fixedRssSafety }),
            startedMonotonicMs: started,
            deadlineMonotonicMs: deadline,
            ...controllerExit,
          }
        : {}),
    });
  }
  return code;
}

export function superviseController({
  worker,
  args,
  deadline,
  stderr,
  terminalMs = FINALIZATION_MS,
  diagnostic = false,
  onVerifiedExit,
  recordFailure = () => {},
  nextOuterChannel,
}) {
  return new Promise((resolve) => {
    const started = monotonicMs();
    const workDeadline = deadline - terminalMs;
    const channel = nextOuterChannel && takeNextOuterChannel(nextOuterChannel);
    const childEnv = channel ? { ...process.env } : undefined;
    if (childEnv) delete childEnv.ZFB_NEXT_OUTER_CHANNEL;
    let child;
    let closeFailure;
    try {
      child = fork(worker, [JSON.stringify({ ...args, workDeadline })], {
        execArgv: [],
        ...(childEnv && { env: childEnv }),
        stdio: ["ignore", "inherit", "inherit", "ipc", ...(channel?.descriptors ?? [])],
      });
    } finally {
      try {
        channel?.close();
      } catch (error) {
        if (!child) throw error;
        closeFailure = error;
      }
    }
    let workTimer;
    let terminalTimer;
    let reason = null;
    let phase = "work";
    let completed = false;
    let reportedExitCode = null;
    let settled = false;
    let attemptedSignal = null;
    const finish = (code, exitSignal = null) => {
      if (settled) return;
      settled = true;
      clearTimeout(workTimer);
      clearTimeout(terminalTimer);
      process.off("SIGTERM", interrupted);
      process.off("SIGINT", interrupted);
      if (!completed) reason ??= "controller exited without reporting completion";
      if (completed && reportedExitCode !== code)
        reason ??= "controller exit differs from reported result";
      if (exitSignal) reason ??= "controller exited on signal " + exitSignal;
      if (diagnostic && monotonicMs() > deadline) reason ??= "terminal deadline reached";
      if (reason) {
        try {
          recordFailure({
            at: new Date().toISOString(),
            phase,
            reason,
            startedMonotonicMs: args.supervisorStartedMs ?? started,
            deadlineMonotonicMs: deadline,
            workDeadlineMonotonicMs: workDeadline,
            terminalBudgetMs: terminalMs,
            elapsedMs: monotonicMs() - (args.supervisorStartedMs ?? started),
            controllerPid: child.pid ?? null,
            reportedExitCode,
            controllerExitCode: code ?? null,
            controllerExitSignal: exitSignal,
            attemptedControllerSignal: attemptedSignal,
          });
        } catch (error) {
          stderr("failed to persist supervisor failure: " + error.message);
        }
      }
      if (!reason && typeof code === "number")
        onVerifiedExit?.({ controllerPid: child.pid, controllerExitMonotonicMs: monotonicMs() });
      resolve(!reason && typeof code === "number" ? code : EXIT.controllerError);
    };
    const finalize = () => {
      clearTimeout(workTimer);
      if (terminalTimer) return;
      phase = "finalizing";
      terminalTimer = setTimeout(
        () => {
          reason ??= "terminal deadline reached";
          attemptedSignal = "SIGKILL";
          stderr(
            `session ${args.label}: terminal deadline reached; terminating controller; cleanup requires verification`,
          );
          // A ChildProcess handle references our direct, unreaped child. Never
          // target a server PID or a process group from this independent timer.
          child.kill("SIGKILL");
          // A task stuck in uninterruptible kernel sleep may outlive SIGKILL.
          // A durable intent still names the session and requires reconciliation.
          if (child.connected) child.disconnect();
          child.unref();
          finish(child.exitCode, child.signalCode);
        },
        Math.max(0, Math.min(deadline - monotonicMs(), terminalMs)),
      );
    };
    const stopWork = (cause) => {
      if (settled) return;
      reason ??= cause;
      stderr(`session ${args.label}: ${cause}`);
      finalize();
      if (child.connected) child.send({ type: "abort", reason }, () => {});
    };
    const interrupted = () => stopWork("supervisor interrupted");
    process.on("SIGTERM", interrupted);
    process.on("SIGINT", interrupted);
    child.on("message", (message) => {
      if (message?.type === "finalizing") finalize();
      if (message?.type === "finished") {
        completed = true;
        reportedExitCode = message.exitCode ?? null;
        finalize();
      }
    });
    child.once("disconnect", () => {
      if (!completed && !settled) stopWork("controller IPC closed before completion");
    });
    child.once("error", (error) => {
      reason ??= error.message;
      if (!child.pid) finish(EXIT.controllerError);
      stderr(`session ${args.label}: controller error: ${error.message}`);
    });
    child.once("exit", finish);
    workTimer = setTimeout(
      () => stopWork("session wall cleanup reserve reached"),
      Math.max(0, workDeadline - monotonicMs()),
    );
    if (closeFailure) stopWork("outer channel cleanup failed: " + closeFailure.message);
  });
}
