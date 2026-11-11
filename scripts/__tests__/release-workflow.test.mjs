// Structural backstop for .github/workflows/release.yml (#4690). The behavior
// of each step body lives in scripts/release/* and is tested there; this suite
// pins the wiring those tests cannot see: no hosted macOS, least-privilege
// permissions, dry-run guards on every mutating step, and the publish modes.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";
import { scriptsDir } from "./helpers/darwin-fixture.mjs";

const repoRoot = join(scriptsDir, "..");
const text = readFileSync(join(repoRoot, ".github/workflows/release.yml"), "utf8");
const workflow = parse(text);
const jobs = workflow.jobs;
const steps = Object.entries(jobs).flatMap(([job, spec]) =>
  (spec.steps ?? []).map((step) => ({ job, step })),
);
const label = ({ job, step }) => `${job}: ${step.name ?? step.uses}`;

const EXPECTED_PERMISSIONS = {
  "release-context": { contents: "read" },
  "source-ci": { contents: "read", checks: "read", statuses: "read" },
  "provenance-preflight": { contents: "read" },
  "admit-darwin": { contents: "read" },
  build: { contents: "read" },
  "build-wasm-md": { contents: "read" },
  "release-assets": { contents: "write" },
  publish: { contents: "write", "id-token": "write" },
  finalize: { contents: "write", "id-token": "write" },
  "smoke-clean-room": { contents: "read" },
  notify: { contents: "read" },
};

// Anything that writes to npm or to the GitHub Release.
const MUTATING = [
  /publish-npm-packages\.sh/,
  /\bnpm publish\b/,
  /\bnpm dist-tag (add|rm)\b/,
  /advance-latest-dist-tag\.sh/,
  /retire-next-dist-tag\.mjs/,
  /gh release upload/,
  /--upload\b/,
  /reconcile-release-assets\.mjs(?![^\n]*\\\n[^\n]*--verify-only)(?![^\n]*--verify-only)/,
];
const isDryRunGuarded = (step) => /inputs\.dry_run != true/.test(String(step.if ?? ""));

describe("release.yml structure", () => {
  it("uses no hosted macOS runner and no fast-Mac / matrix-detection leftovers", () => {
    expect(text).not.toMatch(/macos-/);
    expect(text).not.toMatch(/skip_macos_x64|detect-mac-local|prepare-matrix|mac_local_present/);
    expect(text).not.toMatch(/all-provenance/);
    const targets = jobs.build.strategy.matrix.settings.map((s) => s.target);
    expect(targets.sort()).toEqual([
      "aarch64-unknown-linux-gnu",
      "x86_64-pc-windows-msvc",
      "x86_64-unknown-linux-gnu",
    ]);
    for (const spec of Object.values(jobs)) {
      expect(String(spec["runs-on"] ?? "")).not.toMatch(/macos/);
    }
  });

  it("dispatch inputs are exactly dry_run, release_tag, allow_provenance_downgrade, finalize_only", () => {
    const inputs = workflow.on.workflow_dispatch.inputs;
    expect(Object.keys(inputs).sort()).toEqual([
      "allow_provenance_downgrade",
      "dry_run",
      "finalize_only",
      "release_tag",
    ]);
    expect(inputs.dry_run.default).toBe(false);
    expect(inputs.allow_provenance_downgrade.default).toBe(false);
    expect(inputs.finalize_only.default).toBe(false);
  });

  it("defaults the workflow token to no permissions and grants each job only its own", () => {
    expect(workflow.permissions).toEqual({});
    expect(Object.keys(jobs).sort()).toEqual(Object.keys(EXPECTED_PERMISSIONS).sort());
    for (const [job, expected] of Object.entries(EXPECTED_PERMISSIONS)) {
      expect([job, jobs[job].permissions]).toEqual([job, expected]);
    }
  });

  it("guards every npm/dist-tag/Release-asset write on dry_run", () => {
    const writers = steps.filter(({ step }) =>
      MUTATING.some((re) => re.test(String(step.run ?? ""))),
    );
    expect(writers.length).toBeGreaterThanOrEqual(8);
    const unguarded = writers.filter(({ step }) => !isDryRunGuarded(step)).map(label);
    expect(unguarded).toEqual([]);
  });

  it("never clobbers a Release asset", () => {
    for (const { step } of steps) expect(String(step.run ?? "")).not.toMatch(/--clobber/);
  });

  it("publishes normally in mac-local mode with both owner-Mac carriers", () => {
    const normal = jobs.publish.steps.find((s) => s.name === "Publish to npm");
    expect(normal.run).toBe("bash scripts/publish-npm-packages.sh mac-local");
    expect(normal.env.ZFB_LOCAL_BUILT_PACKAGES).toBe(
      "packages/zfb-darwin-arm64 packages/zfb-darwin-x64",
    );
    expect(normal.env.ZFB_RELEASE_SOURCE_COMMIT).toContain("source_commit");
    expect(normal.env.ZFB_ALLOW_PROVENANCE_DOWNGRADE).toContain(
      "provenance-preflight.outputs.cleared",
    );
    const recovery = jobs.publish.steps.find(
      (s) => s.name === "Publish to npm (recovery — no provenance)",
    );
    expect(recovery.run).toContain(
      ".release-control/scripts/publish-npm-packages.sh recovery-no-provenance",
    );
    expect(recovery.env.ZFB_LOCAL_BUILT_PACKAGES).toBeUndefined();
  });

  it("runs the provenance preflight before any build, never setting the acknowledgement", () => {
    expect(jobs.build.needs).toEqual(
      expect.arrayContaining(["provenance-preflight", "admit-darwin"]),
    );
    const run = jobs["provenance-preflight"].steps.find((s) => s.id === "preflight").run;
    expect(run).toContain(
      "--package @takazudo/zfb-darwin-arm64 --package @takazudo/zfb-darwin-x64",
    );
    expect(text).not.toMatch(/allow_downgrade[^\n]*=\s*true['"]?\s*$/m);
    // The only sources of the acknowledgement are the Release-body marker and the dispatch input.
    const ack = jobs["release-context"].steps.find((s) => s.id === "ack");
    expect(ack.env.DISPATCH_ALLOW).toBe("${{ inputs.allow_provenance_downgrade }}");
  });

  it("places Darwin bytes only from the admitted artifact", () => {
    const placing = steps.filter(({ step }) =>
      /packages\/zfb-darwin-/.test(String(step.run ?? "")),
    );
    expect(placing.map(label)).toEqual([
      "publish: Verify binary mode in platform package tarballs",
    ]);
    const place = jobs.publish.steps.find(
      (s) => s.name === "Place admitted Darwin binaries in platform packages",
    );
    expect(place.run).toContain("place-darwin-carriers.mjs");
    expect(place.run).toContain("darwin-admission/verdict.json");
    expect(jobs.publish.needs).toContain("admit-darwin");
  });

  it("gates publication and finalization on the source commit's CI", () => {
    expect(jobs.publish.needs).toContain("source-ci");
    expect(jobs.finalize.needs).toContain("source-ci");
    const run = jobs["source-ci"].steps.find(
      (s) => s.name === "Verify required CI on the source commit",
    ).run;
    expect(run).toContain('verify-source-ci.mjs --sha "$SOURCE_COMMIT"');
  });

  it("separates finalize-only from build/publish", () => {
    for (const job of ["build", "build-wasm-md", "release-assets", "publish"]) {
      expect([job, jobs[job].if]).toEqual([
        job,
        "${{ needs.release-context.outputs.is_finalize != 'true' }}",
      ]);
    }
    expect(jobs.finalize.if).toBe("${{ needs.release-context.outputs.is_finalize == 'true' }}");
    const finalizeRuns = jobs.finalize.steps.map((s) => String(s.run ?? "")).join("\n");
    expect(finalizeRuns).not.toMatch(
      /publish-npm-packages|npm publish|pnpm publish|cargo|cross build/,
    );
    expect(finalizeRuns).toContain("finalize-gate.mjs");
  });

  it("records the source/control split in release-context outputs", () => {
    expect(Object.keys(jobs["release-context"].outputs)).toEqual(
      expect.arrayContaining([
        "source_commit",
        "source_tree",
        "control_sha",
        "workflow_ref",
        "is_finalize",
      ]),
    );
  });
});
