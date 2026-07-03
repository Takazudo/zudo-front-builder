// Extracted from zfb.mjs so the detection decision can be unit-tested
// without triggering zfb.mjs's top-level exec/spawn side effects. Pure and
// dependency-injectable: production callers can omit every option.
import { readdirSync } from "node:fs";

const MUSL_LOADER_DIR = "/lib";
const MUSL_LOADER_PREFIX = "ld-musl-";

// True if a musl dynamic loader file is present (e.g. Alpine's
// /lib/ld-musl-x86_64.so.1). Reading the directory and matching the prefix
// covers every arch's loader filename with one probe instead of hardcoding
// an existsSync path per arch. Mirrors the loader-file probe used by the
// widely-adopted `detect-libc` npm package.
function hasMuslLoaderFile(readdirSyncFn) {
  try {
    return readdirSyncFn(MUSL_LOADER_DIR).some((name) => name.startsWith(MUSL_LOADER_PREFIX));
  } catch {
    return false;
  }
}

function defaultGetReport() {
  return process.report?.getReport?.();
}

// True if Node's process.report exposes a glibc runtime version. glibc-built
// Node populates header.glibcVersionRuntime; musl-built Node does not. Used
// only as a fallback signal when the loader-file probe above is inconclusive.
function hasGlibcVersionRuntime(getReportFn) {
  try {
    return Boolean(getReportFn()?.header?.glibcVersionRuntime);
  } catch {
    return false;
  }
}

/**
 * Detects musl libc (e.g. Alpine) on Linux, where the gnu-built platform
 * binary would otherwise fail with an opaque dynamic-loader exec error
 * instead of a clear "not supported" message. Always returns false on
 * non-linux platforms (darwin, win32) — zero behavior change there.
 */
export function detectMuslLinux({
  platform = process.platform,
  readdirSyncFn = readdirSync,
  getReportFn = defaultGetReport,
} = {}) {
  if (platform !== "linux") return false;
  // The glibc runtime report is authoritative when present: a Node process
  // that reports a glibc version is definitely glibc-linked and will work,
  // regardless of an unrelated musl loader file sitting on the filesystem
  // (e.g. a `musl` package installed on Debian/Ubuntu for other reasons).
  if (hasGlibcVersionRuntime(getReportFn)) return false;
  return hasMuslLoaderFile(readdirSyncFn);
}
