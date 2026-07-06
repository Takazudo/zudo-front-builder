/**
 * Build an actionable stderr message for a `spawnSync()` launch failure
 * (`result.error` set — e.g. ENOENT/EACCES trying to invoke the resolved
 * `@takazudo/zfb` launcher). Extracted as a pure function (no top-level
 * side effects, unlike create-zfb.mjs) so it can be unit tested directly,
 * mirroring the version-check.mjs pattern in this package.
 *
 * Mirrors packages/zfb/bin/zfb.mjs's `child.on("error", ...)` messaging
 * (issues #447/#441) so both wrappers give the same reinstall guidance.
 *
 * @param {NodeJS.ErrnoException} error
 * @param {string} zfbBin - absolute path to the resolved bin/zfb.mjs launcher
 * @returns {string}
 */
export function formatSpawnErrorMessage(error, zfbBin) {
  if (error.code === "EACCES") {
    return (
      "[create-zfb] not executable; was the install corrupt?\n" +
      `      ${zfbBin}\n` +
      "      Try reinstalling: npm install --include=optional\n"
    );
  }
  return `[create-zfb] failed to spawn @takazudo/zfb: ${error.message}\n` + `      ${zfbBin}\n`;
}
