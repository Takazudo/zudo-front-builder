function sleep(ms) {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

function errorClass(error) {
  return error && typeof error === "object" && error.constructor?.name
    ? error.constructor.name
    : typeof error;
}

function formatError(error) {
  if (error instanceof Error) return `${error.name}: ${error.message}`;
  return String(error);
}

function formatStatus(status) {
  return typeof status === "string" ? status : JSON.stringify(status);
}

function isProbeResult(result) {
  return (
    result !== null &&
    typeof result === "object" &&
    typeof result.ok === "boolean" &&
    (typeof result.status === "string" || typeof result.status === "number") &&
    Array.isArray(result.markers) &&
    result.markers.every((marker) => typeof marker === "string") &&
    typeof result.note === "string"
  );
}

function attachDiagnostics(error, diagnostics) {
  if (!error || typeof error !== "object") return;
  try {
    error.timeline = diagnostics.timeline;
    error.lastNonOkStatus = diagnostics.lastNonOkStatus;
    error.lastThrownError = diagnostics.lastThrownError;
  } catch {
    // Fatal errors must retain their original identity even when immutable.
  }
}

/**
 * Poll until a structured probe succeeds or the deadline expires.
 *
 * A probe returns { ok, status, markers, note }. Returned results clear the
 * active timeout cause; the last thrown error remains available separately
 * for diagnostics. `now` and `sleep` make timeout behavior deterministic in
 * unit tests.
 */
export async function poll(description, probe, options = {}) {
  const {
    timeoutMs = 90_000,
    intervalMs = 300,
    isFatal = () => false,
    now = Date.now,
    sleep: wait = sleep,
  } = options;

  const deadline = now() + timeoutMs;
  const timeline = [];
  let lastError;
  let lastThrownError;
  let lastNonOkStatus;

  while (now() < deadline) {
    const timestamp = new Date(now()).toISOString();
    let result;
    try {
      result = await probe();
    } catch (error) {
      lastThrownError = error;
      const entry = {
        timestamp,
        status: "threw",
        markers: [],
        thrownErrorClass: errorClass(error),
        note: formatError(error),
      };
      timeline.push(entry);

      if (isFatal(error)) {
        attachDiagnostics(error, { timeline, lastNonOkStatus, lastThrownError });
        throw error;
      }

      lastError = error;
      await wait(intervalMs);
      continue;
    }

    if (!isProbeResult(result)) {
      throw new TypeError("poll probe must return { ok, status, markers, note }");
    }

    // Any non-throwing result supersedes an earlier exception as the active
    // timeout cause, including a structured result that is still not ready.
    lastError = undefined;
    const entry = {
      timestamp,
      status: result.status,
      markers: [...result.markers],
      thrownErrorClass: null,
      note: result.note,
    };
    timeline.push(entry);

    if (result.ok) return { ...result, timeline };

    lastNonOkStatus = {
      status: result.status,
      markers: [...result.markers],
      note: result.note,
    };
    await wait(intervalMs);
  }

  const lines = [`timed out waiting for ${description}`];
  if (lastNonOkStatus) {
    lines.push(
      `Last non-OK status: ${formatStatus(lastNonOkStatus.status)}${lastNonOkStatus.note ? ` (${lastNonOkStatus.note})` : ""}`,
    );
    if (lastNonOkStatus.markers.length > 0) {
      lines.push(`Last markers found: ${JSON.stringify(lastNonOkStatus.markers)}`);
    }
  } else {
    lines.push("Last non-OK status: none");
  }

  if (lastThrownError) {
    lines.push(`Last thrown error: ${formatError(lastThrownError)}`);
    if (lastThrownError?.cause !== undefined) {
      lines.push(`cause: ${formatError(lastThrownError.cause)}`);
    }
  } else {
    lines.push("Last thrown error: none");
  }

  lines.push(`Probe timeline (${timeline.length} probes):`);
  for (const entry of timeline) {
    lines.push(
      `  ${entry.timestamp} status=${formatStatus(entry.status)} markers=${JSON.stringify(entry.markers)} thrownErrorClass=${entry.thrownErrorClass ?? "none"}${entry.note ? ` note=${JSON.stringify(entry.note)}` : ""}`,
    );
  }

  const timeoutError = new Error(
    lines.join("\n"),
    lastError === undefined ? undefined : { cause: lastError },
  );
  timeoutError.timeline = timeline;
  timeoutError.lastNonOkStatus = lastNonOkStatus;
  timeoutError.lastThrownError = lastThrownError;
  throw timeoutError;
}
