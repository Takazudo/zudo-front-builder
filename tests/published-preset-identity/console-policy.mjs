// Console oracle policy for the positive browser case (#4177). See README.md.
//
// The positive case is a controlled fixture: a pinned install, one host Island,
// and no third-party scripts. A warning there is an unexplained behavior change
// in the published presets or the runtime, so warnings fail exactly like errors.
// There is deliberately no allowlist; a new warning is triaged, not suppressed.

export const FATAL_CONSOLE_TYPES = Object.freeze(["error", "warning"]);

export function isFatalConsoleMessage(type) {
  return FATAL_CONSOLE_TYPES.includes(type);
}

export function consoleOracleFailures({ console: messages = [], pageErrors = [] }) {
  const failures = [];
  if (pageErrors.length) failures.push(`page errors: ${pageErrors.join(" | ")}`);
  if (messages.length) failures.push(`console errors/warnings: ${messages.join(" | ")}`);
  return failures;
}
