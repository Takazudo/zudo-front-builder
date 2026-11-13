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
  pack: { contents: "read" },
  // actions: read — receipt lookup (publish-receipts.mjs, #4716).
  publish: { contents: "read", "id-token": "write", actions: "read" },
  "md-wasm-asset": { contents: "write" },
  finalize: { contents: "read", "id-token": "write", actions: "read" },
  "finalize-assets": { contents: "write" },
  "smoke-clean-room": { contents: "read" },
  notify: { contents: "read" },
};

// Anything that writes to npm or to the GitHub Release.
const MUTATING = [
  /publish-npm-packages\.sh/,
  /\bnpm publish\b/,
  /\bnpm dist-tag (add|rm)\b/,
  /advance-latest-dist-tag\.sh/,
  /promote-release-channel\.sh/,
  /retire-next-dist-tag\.mjs/,
  /gh release upload/,
  /--upload\b/,
  /reconcile-release-assets\.mjs(?![^\n]*\\\n[^\n]*--verify-only)(?![^\n]*--verify-only)/,
];
const isDryRunGuarded = ({ job, step }) =>
  /inputs\.dry_run != true/.test(String(step.if ?? "")) ||
  /inputs\.dry_run != true/.test(String(jobs[job].if ?? ""));

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
    const unguarded = writers.filter((entry) => !isDryRunGuarded(entry)).map(label);
    expect(unguarded).toEqual([]);
  });

  it("never clobbers a Release asset", () => {
    for (const { step } of steps) expect(String(step.run ?? "")).not.toMatch(/--clobber/);
  });

  it("publishes pre-packed tarballs by mode, with both owner-Mac carriers unattested", () => {
    const normal = jobs.publish.steps.find((s) => s.name === "Publish to npm");
    expect(normal.run).toBe("bash scripts/publish-npm-packages.sh mac-local");
    expect(normal.if).toContain("publish_mode == 'mac-local'");
    expect(normal.env.ZFB_PUBLISH_TARBALLS_DIR).toContain("npm-tarballs");
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
    expect(recovery.if).toContain("publish_mode == 'recovery-no-provenance'");
    expect(recovery.env.ZFB_LOCAL_BUILT_PACKAGES).toBeUndefined();
  });

  it("keeps id-token away from install/build/package code (#4690)", () => {
    for (const [name, job] of Object.entries(jobs)) {
      if (job.permissions?.["id-token"] !== "write") continue;
      const runs = (job.steps ?? []).map((s) => String(s.run ?? "")).join("\n");
      expect([name, runs]).not.toEqual([
        name,
        expect.stringMatching(
          /pnpm|npm (ci|install)(?! -g npm@)|npm pack|cargo|cross build|prepublishOnly/,
        ),
      ]);
      expect([
        name,
        (job.steps ?? []).some((s) => String(s.uses ?? "").startsWith("pnpm/action-setup")),
      ]).toEqual([name, false]);
      // Only the control commit is checked out, without persisted credentials.
      for (const step of (job.steps ?? []).filter((s) =>
        String(s.uses ?? "").startsWith("actions/checkout"),
      )) {
        expect([name, step.with.ref, step.with["persist-credentials"]]).toEqual([
          name,
          "${{ github.sha }}",
          false,
        ]);
      }
      expect([name, job.permissions.contents]).toEqual([name, "read"]);
    }
    const pack = jobs.pack.steps.find((s) => s.name === "Pack npm tarballs");
    expect(pack.run).toContain("pack-release-tarballs.mjs");
    expect(jobs.publish.needs).toEqual(expect.arrayContaining(["pack", "release-assets"]));
  });

  it("sets persist-credentials: false on every checkout", () => {
    for (const { job, step } of steps.filter(({ step }) =>
      String(step.uses ?? "").startsWith("actions/checkout"),
    )) {
      expect([job, step.with?.["persist-credentials"]]).toEqual([job, false]);
    }
  });

  it("keys concurrency on the normalized tag and isolates dry runs", () => {
    expect(workflow.concurrency.group).toBe(
      "release-${{ inputs.dry_run && 'dry-run-' || '' }}${{ inputs.release_tag || github.event.release.tag_name || github.ref_name }}",
    );
    expect(workflow.concurrency["cancel-in-progress"]).toBe(false);
  });

  it("uses honest provenance for a recovery whose control commit is the source", () => {
    const run = jobs["release-context"].steps.find((s) => s.id === "mode").run;
    expect(run).toContain(
      'if [[ "$IS_RECOVERY" == "true" && "$SOURCE_COMMIT" != "$CONTROL_SHA" ]]',
    );
    expect(jobs["release-context"].outputs.publish_mode).toBe(
      "${{ steps.mode.outputs.publish_mode }}",
    );
  });

  it("decides the prerelease dual-tag with the fail-closed helper", () => {
    for (const job of ["publish", "finalize"]) {
      const runs = jobs[job].steps.map((s) => String(s.run ?? "")).join("\n");
      expect(runs).toContain("resolve-also-latest.mjs");
      expect(runs).not.toMatch(/dist-tags\.latest 2>\/dev\/null \|\| echo/);
    }
  });

  it("re-inspects tarball content in the publish job with its own inputs (NEW-m1)", () => {
    for (const name of ["Publish to npm", "Publish to npm (recovery — no provenance)"]) {
      const step = jobs.publish.steps.find((s) => s.name === name);
      expect([name, step.env.ZFB_RELEASE_VERSION]).toEqual([
        name,
        "${{ needs.release-context.outputs.release_version }}",
      ]);
      expect(step.env.ZFB_DARWIN_ADMISSION_VERDICT).toContain("darwin-admission/verdict.json");
      expect(step.env.ZFB_MD_WASM_BUILD_MANIFEST).toContain("wasm-md-shipped-artifacts");
    }
    const download = jobs.publish.steps.find(
      (s) => s.name === "Download @takazudo/zfb-md-wasm manifest artifact",
    );
    expect(download.with.name).toBe("wasm-md-shipped-artifacts");
  });

  it("reads the provenance policy from main and requires the source on main (NEW-m2)", () => {
    const preflight = jobs["provenance-preflight"].steps.find((s) => s.id === "preflight").run;
    expect(preflight).toContain(
      "git fetch --no-tags --depth=1 origin +refs/heads/main:refs/remotes/origin/main",
    );
    expect(preflight).not.toContain("--policy-ref");
    const verify = jobs["release-context"].steps.find((s) => s.id === "verify");
    expect(verify.env.DRY_RUN).toBe("${{ inputs.dry_run }}");
    expect(verify.run).toContain(
      "git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main",
    );
  });

  it("orders release-assets after source CI", () => {
    expect(jobs["release-assets"].needs).toContain("source-ci");
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

  it("places Darwin bytes only from the admitted artifact, in the pack job", () => {
    const placing = steps.filter(({ step }) =>
      /packages\/zfb-darwin-/.test(String(step.run ?? "")),
    );
    expect(placing.map(label)).toEqual(["pack: Pack npm tarballs"]);
    const place = jobs.pack.steps.find(
      (s) => s.name === "Place admitted Darwin binaries in platform packages",
    );
    expect(place.run).toContain("place-darwin-carriers.mjs");
    expect(place.run).toContain("darwin-admission/verdict.json");
    expect(jobs.pack.needs).toContain("admit-darwin");
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
    for (const job of [
      "build",
      "build-wasm-md",
      "release-assets",
      "pack",
      "publish",
      "md-wasm-asset",
    ]) {
      expect([job, String(jobs[job].if)]).toEqual([
        job,
        expect.stringContaining("needs.release-context.outputs.is_finalize != 'true'"),
      ]);
    }
    expect(jobs.finalize.if).toBe("${{ needs.release-context.outputs.is_finalize == 'true' }}");
    const finalizeRuns = jobs.finalize.steps.map((s) => String(s.run ?? "")).join("\n");
    expect(finalizeRuns).not.toMatch(
      /publish-npm-packages|npm publish|pnpm publish|cargo|cross build/,
    );
    expect(finalizeRuns).toContain("finalize-gate.mjs");
  });

  it("moves channel tags only in the post-verification promotion step (#4717)", () => {
    const stepIndex = (job, name) => jobs[job].steps.findIndex((s) => s.name === name);
    const PROMOTE = "Promote channel dist-tag (all packages verified)";
    const SWEEP = "Sweep stale 'next' and 'release-pending' dist-tags";
    // The only writers of `latest` / `next` are the promotion steps.
    const advancing = steps.filter(({ step }) =>
      /advance-latest-dist-tag\.sh|promote-release-channel\.sh|npm dist-tag add/.test(
        String(step.run ?? ""),
      ),
    );
    expect(advancing.map(label).sort()).toEqual([`finalize: ${PROMOTE}`, `publish: ${PROMOTE}`]);
    for (const [job, after] of [
      ["publish", "Verify npm provenance expectations"],
      ["finalize", "Verify npm provenance expectations"],
    ]) {
      const promote = jobs[job].steps[stepIndex(job, PROMOTE)];
      expect([job, promote.run]).toEqual([
        job,
        'bash scripts/promote-release-channel.sh "$RELEASE_VERSION"',
      ]);
      // No status function: runs only when every earlier step (the verifier) passed.
      expect([job, promote.if]).toEqual([job, "${{ inputs.dry_run != true }}"]);
      expect(promote.env.ZFB_RELEASE_SOURCE_COMMIT).toContain("source_commit");
      expect(promote.env.ZFB_DARWIN_ADMISSION_VERDICT).toContain("darwin-admission/verdict.json");
      expect(stepIndex(job, PROMOTE)).toBeGreaterThan(stepIndex(job, after));
      expect(stepIndex(job, SWEEP)).toBeGreaterThan(stepIndex(job, PROMOTE));
      expect(jobs[job].steps[stepIndex(job, SWEEP)].if).toBe(
        "${{ always() && inputs.dry_run != true }}",
      );
    }
    expect(stepIndex("finalize", PROMOTE)).toBeGreaterThan(
      stepIndex(
        "finalize",
        "Finalization gate (all packages present-verified, build assets complete)",
      ),
    );
  });

  it("publishes every package under the release-pending holding tag (#4717)", () => {
    const script = readFileSync(join(repoRoot, "scripts/publish-npm-packages.sh"), "utf8");
    expect(script).toMatch(/^HOLDING_DIST_TAG="release-pending"$/m);
    const tags = [...script.matchAll(/npm publish [^\n]*--tag (\S+)/g)].map((m) => m[1]);
    expect(tags.length).toBeGreaterThan(0);
    expect(new Set(tags)).toEqual(new Set(['"$HOLDING_DIST_TAG"']));
    expect(script).not.toMatch(/advance-latest-dist-tag|maybe_advance_latest|\$DIST_TAG/);
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
