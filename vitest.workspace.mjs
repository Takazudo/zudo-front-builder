import { defaultExclude, defineWorkspace } from "vitest/config";

// Two projects over the SAME root-level scripts/**/__tests__ corpus, split so
// the 5000 ms default stays the hang guardrail everywhere except the suites
// that genuinely await real child processes. vitest 2.1.9 has no
// `test.projects`, so this is a vitest 2 workspace file; both projects
// `extends` the single root config, which still owns include and environment.
//
// Why only these four: vitest's testTimeout is a timer, so it can only fire on
// a test that yields to the event loop. Suites that block on execFileSync are
// immune to it at any host load -- measured under #3061, changelog-layout ran a
// single test for 30.1 s on a saturated host and passed. So the deadline only
// ever governs the suites that `await` a subprocess. A transitive import-graph
// audit of all 19 scripts/__tests__ files agrees with the measurement: these
// four, and nothing else, reach child_process on an awaited path.
//
// To add a suite here, check that it awaits (not execFileSync-blocks) a child
// process, then add it to this one list -- it is the only place either project
// names a suite, so the two cannot start overlapping and run it twice.
const SUBPROCESS_SUITES = [
  "scripts/__tests__/docs-dev-supervisor.test.mjs",
  "scripts/__tests__/harvest-supervisor-timelines.test.mjs",
  "scripts/__tests__/supervisor-watch-handoff.test.mjs",
  "scripts/__tests__/supervisor-watch.test.mjs",
];

// Both projects are carved out with `exclude` rather than `include`, because
// `extends` merges through vite's mergeConfig, which CONCATENATES arrays: a
// project-level `include` is appended to the root config's pattern instead of
// replacing it, so the subprocess project would pick up the whole corpus and
// every non-subprocess suite would run twice. `exclude` is additive by nature,
// so concatenation is the behaviour it wants. defaultExclude must be re-stated
// for the same reason it always must -- vitest drops its defaults once the key
// is present at all. The `**/` before the extglob mirrors the root config's
// `scripts/**/__tests__/**/*.test.mjs`: without it the negation only covers
// `__tests__`'s direct children, so a suite in a subdirectory would be excluded
// from neither project and run twice.
const SUBPROCESS_NAMES = SUBPROCESS_SUITES.map((path) =>
  path.replace(/^.*\//, "").replace(/\.test\.mjs$/, ""),
);
const EVERYTHING_ELSE = `scripts/**/__tests__/**/!(${SUBPROCESS_NAMES.join("|")}).test.mjs`;

export default defineWorkspace([
  {
    extends: "./vitest.config.mjs",
    test: {
      name: "scripts",
      exclude: [...defaultExclude, ...SUBPROCESS_SUITES],
    },
  },
  {
    extends: "./vitest.config.mjs",
    test: {
      name: "scripts-subprocess",
      exclude: [...defaultExclude, EVERYTHING_ELSE],
      // 90 s is the outer project guard for the four sequential 16 s
      // phase-naming waits in docs-dev-supervisor's SIGINT test plus 1 s cleanup:
      // 65 s / 0.75 ~= 86.7 s, rounded up in the existing 10 s sizing step.
      // It stays above the inner waits so they name their phase rather than
      // firing a bare project-level "Test timed out". The loaded 27.9 s worst
      // case measured over 10 runs (one `yes` burner per core on a 10-core Mac)
      // -- #3058, findings on #3061 -- remains below this outer guard. The
      // loaded median is 3-10 s and the quiet-host max is 1.1 s, so this is a
      // hang guardrail, not a budget. Harvest keeps its tighter 20 s
      // describe-level timeout; docs-dev-supervisor's describe-level timeout
      // matches this outer guard so its inner waits still name their phase.
      testTimeout: 90_000,
    },
  },
]);
