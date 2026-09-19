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
      // 60 s is 2.1x the 27.9 s worst case measured over 10 loaded runs of
      // these suites (one `yes` burner per core on a 10-core Mac) -- #3058,
      // findings on #3061. Sized from a run with a RAISED timeout already in
      // place, not from the 10.9 s tail seen at the old 5 s default: aborting
      // at 5 s truncated every run, so the pre-fix distribution understates
      // the tail by roughly half. The loaded median is 3-10 s and the
      // quiet-host max is 1.1 s, so this is a hang guardrail, not a budget.
      // Tests under the two describe-level timeouts in these files (20 s in
      // harvest-supervisor-timelines, 60 s in docs-dev-supervisor) keep their
      // own value and stay the tighter, phase-naming bound.
      testTimeout: 60_000,
    },
  },
]);
