// Loads scripts/__tests__/fixtures/rest-job-log-capture.log — a REAL,
// trimmed capture of the body the harvester now parses (#2931).
//
// Provenance: captured 2026-09-07 with
//
//   gh api repos/Takazudo/zudo-front-builder/actions/jobs/101650051136/logs
//
// (the `health` job of run 34092877454, `main` @ b30ace4c, conclusion
// success, 10,070 lines). Trimmed to lines 1-8 and 1036-1064 of that body:
// the runner-header block and the vitest tail that carries the three
// `[supervisor-timeline]` records. Nothing else was altered, so the
// timestamps jump across the cut — that gap is the trim, not a bug.
//
// Why a capture and not a hand-written sample: the record line is zfb's own
// contract (hand-authored corpus in supervisor-timeline-samples.txt is
// correct there), but the job-log ENVELOPE around it is GitHub's, and a
// guess at it fails silently — `parseTimelines` would simply find nothing,
// every run would report `lines=0`, and the weekly watch would return a
// confident `no-data` verdict while the lane went dark.
//
// Properties of the real envelope that a hand-written guess gets wrong, and
// that this file therefore carries verbatim:
//   - a leading UTF-8 BOM on the very first line;
//   - a `<ISO timestamp with 7 fractional digits>Z ` prefix per line and NO
//     `<job>\t<step>\t` prefix (that one was `gh run view --log`'s, added
//     client-side by the porcelain this task removed);
//   - `pnpm -r`'s own `. test: ` package label ahead of the tag;
//   - raw ANSI SGR escapes and non-ASCII glyphs in the surrounding noise.

import { readFileSync } from "node:fs";

const CAPTURE_URL = new URL("./rest-job-log-capture.log", import.meta.url);

/** The captured job-log body, verbatim. */
export const REST_JOB_LOG_CAPTURE = readFileSync(CAPTURE_URL, "utf8");

const RECORD_LINE = REST_JOB_LOG_CAPTURE.split("\n").find((line) =>
  line.includes("[supervisor-timeline] case="),
);

if (RECORD_LINE === undefined) {
  throw new Error(`no [supervisor-timeline] record in ${CAPTURE_URL}`);
}

/**
 * Everything the real envelope puts ahead of the tag on a record line, taken
 * from the capture rather than reconstructed — so a synthetic log built with
 * it is wrapped in bytes GitHub actually emitted.
 */
export const REST_LOG_PREFIX = RECORD_LINE.slice(0, RECORD_LINE.indexOf("[supervisor-timeline]"));
