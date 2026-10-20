import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vite-plus/test";
import { main, readGitBlob } from "../rebaseline-island-size-linux.mjs";
import {
  ceilingsOf,
  makeGitCheckout,
  repoRoot,
  sha,
  writeRuns,
} from "./fixtures/island-size-checkout.mjs";
import { rustlsSecurityLock } from "./fixtures/rustls-security-transition.mjs";
import { reviewedCargoLock } from "./fixtures/reviewed-cargo-transition.mjs";

// Real reviewed lock bytes and the actual CLI entry point, with synthetic
// measurement runs in a disposable checkout. This is not native size evidence.
describe.each([
  ["Rolldown", reviewedCargoLock, 46, 16],
  ["rustls security", rustlsSecurityLock, 0, 1],
])(
  "%s reviewed producer transition CLI with git provenance",
  (_name, fixture, changedCount, removedCount) => {
    const roots = [];
    const lock = fixture();
    const review = JSON.parse(lock.reviewedTransitionText);
    const CONTRACT = "research/v3-island-size/decision-linux-x64.json";
    const README = "research/v3-island-size/README.md";
    const OUTPUTS = [CONTRACT, README, "scripts/__tests__/island-size-budget.test.mjs"];
    const live = JSON.parse(readFileSync(join(repoRoot, CONTRACT), "utf8"));
    let repo;
    let artifact;
    let pinned;
    let saved;
    const at = (file) => join(repo.root, file);

    beforeAll(() => {
      repo = makeGitCheckout(roots, {
        oldCargoLock: lock.oldText,
        newCargoLock: lock.newText,
        prepare(checkout) {
          artifact = join(checkout.root, "artifact");
          writeRuns(artifact, Object.keys(live.ceilings.workspace), (mode) => ({
            sourceSha: "b".repeat(40),
            mode,
            platform: { os: "linux", arch: "x64" },
            fixtureVersion: live.fixtureVersion,
            fixtureSha256: checkout.fixtureSha256,
            runnerSha256: checkout.runnerSha256,
            packageVersion: checkout.version,
            runtimePackageVersion: checkout.version,
            honoPackageVersion: live.toolchain.honoVersion,
            esbuildVersion: live.toolchain.esbuildVersion,
            nodeVersion: live.toolchain.nodeVersion,
            zlibVersion: live.toolchain.zlibVersion,
            compression: live.compression,
            repeats: 2,
            pnpmLockSha256: checkout.pnpmLockSha256,
            dependencyFingerprint: {
              policyVersion: checkout.dependencyFingerprint.policyVersion,
              sha256: checkout.dependencyFingerprint.sha256,
            },
            cargoLockSha256: checkout.cargoLockSha256,
          }));
          pinned = structuredClone(live);
          pinned.ceilings = ceilingsOf(artifact);
          pinned.fixtureSha256 = { ...checkout.fixtureSha256 };
          pinned.runnerSha256 = { ...checkout.runnerSha256 };
          pinned.toolchain.packageVersion = checkout.version;
          pinned.toolchain.pnpmLockSha256 = checkout.pnpmLockSha256;
          pinned.toolchain.cargoLockSha256 = lock.expectedOldSha;
          pinned.dependencyFingerprint = { ...checkout.dependencyFingerprint };
          writeFileSync(join(checkout.root, CONTRACT), JSON.stringify(pinned, null, 2) + "\n");
          writeFileSync(join(checkout.root, "review.json"), lock.reviewedTransitionText);
        },
      });
      saved = Object.fromEntries(OUTPUTS.map((file) => [file, readFileSync(at(file))]));
    });
    afterEach(() => {
      for (const [file, bytes] of Object.entries(saved)) writeFileSync(at(file), bytes);
      writeFileSync(at("review.json"), lock.reviewedTransitionText);
      vi.restoreAllMocks();
    });
    afterAll(() => {
      for (const directory of roots) rmSync(directory, { recursive: true, force: true });
    });
    const args = () =>
      Object.entries({
        artifact,
        "run-id": "51",
        "job-id": "52",
        "head-sha": "c".repeat(40),
        reason: "cargo-producer-lock",
        "expected-old-cargo-sha": lock.expectedOldSha,
        "expected-new-cargo-sha": lock.expectedNewSha,
        "old-cargo-ref": "HEAD:Cargo.lock",
        "added-packages": lock.added.join(","),
        "reviewed-cargo-transition": at("review.json"),
      }).flatMap(([key, value]) => ["--" + key, value]);

    it("keeps immutable git provenance and the reviewed proof without moving ceilings", async () => {
      vi.spyOn(console, "log").mockImplementation(() => {});
      await main(args(), repo.root);
      const next = JSON.parse(readFileSync(at(CONTRACT), "utf8"));
      expect(next.ceilings).toEqual(pinned.ceilings);
      expect(next.allowance).toEqual(pinned.allowance);
      expect(next.toolchain).toEqual({
        ...pinned.toolchain,
        cargoLockSha256: lock.expectedNewSha,
      });
      expect(next.baselineEvidence.cargoTransition).toMatchObject({
        oldSha: lock.expectedOldSha,
        newSha: lock.expectedNewSha,
        oldLockSource: { commit: repo.commit, path: "Cargo.lock", sha256: lock.expectedOldSha },
        review: {
          id: review.id,
          manifestSha256: sha(lock.reviewedTransitionText),
          sourceSha: review.review?.sourceSha ?? review.sourceSha,
        },
      });
      expect(next.baselineEvidence.cargoTransition.changed).toHaveLength(changedCount);
      expect(next.baselineEvidence.cargoTransition.removed).toHaveLength(removedCount);
      expect(next.baselineEvidence.repinnedFrom.contract).toEqual({
        sha256: sha(saved[CONTRACT]),
        commit: repo.commit,
        path: CONTRACT,
      });
      const readme = readFileSync(at(README), "utf8");
      expect(readme).not.toContain("[object Object]");
      expect(readme).toContain("proved against `" + repo.commit + ":Cargo.lock`");
      expect(readme).toContain(`${changedCount} changed records`);
      expect(readme).toContain("non-additive");
    });

    it("uses the original old-lock bytes when a local replacement blob exists", async () => {
      vi.spyOn(console, "log").mockImplementation(() => {});
      const original = repo.git("rev-parse", "HEAD:Cargo.lock");
      const replacement = repo.git("hash-object", "-w", "Cargo.lock");
      repo.git("replace", original, replacement);
      try {
        expect(repo.git("show", "HEAD:Cargo.lock")).toBe(lock.newText.trim());
        await main(args(), repo.root);
        expect(
          JSON.parse(readFileSync(at(CONTRACT), "utf8")).baselineEvidence.cargoTransition
            .oldLockSource,
        ).toEqual({
          commit: repo.commit,
          path: "Cargo.lock",
          sha256: lock.expectedOldSha,
        });
      } finally {
        repo.git("replace", "-d", original);
      }
    });

    it.each(["blob", "tree", "commit"])(
      "ignores %s replacements when reading a cited commit",
      (kind) => {
        const isolated = makeGitCheckout(roots, {
          oldCargoLock: lock.oldText,
          newCargoLock: lock.newText,
        });
        isolated.git("add", "Cargo.lock");
        const tree = isolated.git("write-tree");
        const replacementCommit = isolated.git(
          "commit-tree",
          tree,
          "-p",
          isolated.commit,
          "-m",
          "replacement fixture",
        );
        const object = (commit) =>
          kind === "commit"
            ? commit
            : isolated.git("rev-parse", commit + (kind === "tree" ? "^{tree}" : ":Cargo.lock"));
        isolated.git("replace", object(isolated.commit), object(replacementCommit));
        expect(isolated.git("show", isolated.commit + ":Cargo.lock")).toBe(lock.newText.trim());
        const blob = readGitBlob(isolated.root, isolated.commit, "Cargo.lock");
        expect(blob.commit).toBe(isolated.commit);
        expect(blob.bytes.toString()).toBe(lock.oldText);
      },
    );

    it("refuses a corrupt reviewed manifest before mutating any outputs", async () => {
      vi.spyOn(console, "log").mockImplementation(() => {});
      writeFileSync(at("review.json"), lock.reviewedTransitionText + "\n");
      await expect(main(args(), repo.root)).rejects.toThrow(
        /manifest digest differs|decision differs/,
      );
      for (const file of OUTPUTS) expect(readFileSync(at(file)).equals(saved[file])).toBe(true);
    });
  },
);
