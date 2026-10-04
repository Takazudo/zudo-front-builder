import { defaultExclude, defineConfig } from "vitest/config";

// Root-level config for tests over root-level scripts/** (e.g.
// scripts/showcase-inject-banner.mjs, issue #2282). Every other package in
// this workspace (packages/*) carries its own scoped vitest.config — this
// one is scoped the same way, to scripts/**/__tests__, so it never picks up
// those packages' test files (which may need a different environment, e.g.
// happy-dom for zfb-runtime).
//
// Two projects over the SAME root-level corpus, split so the 5000 ms default
// stays the hang guardrail everywhere except the suites that genuinely await
// real child processes.
//
// Why only these four: they, and nothing else, reach child_process on an
// awaited path (a transitive import-graph audit of the scripts/__tests__
// files), so only they wait out multi-second subprocess phases. Under Vitest 2
// a suite that blocked on execFileSync was immune to testTimeout -- #3061
// measured changelog-layout at 30.1 s on a saturated host, passing. Vitest 4+
// also fails a test whose elapsed time reached the timeout once it returns, so
// 5 s now bounds those synchronous suites too (see
// research/toolchain-modernization/STAGE-3-VITEST.md).
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

// The projects are inline and do not inherit this root config, so each owns
// its include/exclude outright: nothing is merged or concatenated into them,
// and `scripts` excludes exactly the list `scripts-subprocess` includes.
// defaultExclude must be re-stated because vitest drops its defaults once the
// key is present at all.
export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "scripts",
          environment: "node",
          include: ["scripts/**/__tests__/**/*.test.mjs"],
          exclude: [...defaultExclude, ...SUBPROCESS_SUITES],
        },
      },
      {
        test: {
          name: "scripts-subprocess",
          environment: "node",
          include: SUBPROCESS_SUITES,
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
    ],
  },
});
