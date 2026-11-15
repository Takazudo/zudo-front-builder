import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";
import {
  classifyDocsSiteChanges,
  detectDocsSiteChanges,
  listPullRequestFiles,
  pathMatchesDocsSite,
  readDocsSitePatterns,
} from "../docs-site-changes.mjs";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const patterns = readDocsSitePatterns(
  readFileSync(join(rootDir, ".github/path-filters/docs-site.yml"), "utf8"),
);
const docsChecks = parse(readFileSync(join(rootDir, ".github/workflows/docs-checks.yml"), "utf8"));

const response = (body, ok = true, status = 200) => ({
  ok,
  status,
  json: async () => body,
});

describe("docs-site path rules", () => {
  it("matches the reviewed generator inputs and not ordinary product sources", () => {
    expect(patterns).toEqual(
      expect.arrayContaining([
        "docs/**",
        "tests/docs-wind-preview/**",
        "scripts/__tests__/generate-wind-reference.test.mjs",
        "crates/zudo-wind/catalog/zudo-wind-catalog.v1.json",
        "tests/wind-compatibility/inventory.v1.json",
        "tests/wind-compatibility/profile.json",
        "scripts/wind-compatibility/inventory.mjs",
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        ".npmrc",
      ]),
    );
    for (const path of [
      "crates/zfb/src/lib.rs",
      "packages/zfb/package.json",
      "Cargo.toml",
      "Cargo.lock",
      "rust-toolchain.toml",
      "scripts/release/coordinate-release.mjs",
    ]) {
      expect(patterns.some((pattern) => pathMatchesDocsSite(pattern, path))).toBe(false);
    }
    for (const path of [
      "docs/src/content/docs/index.mdx",
      "docs/package.json",
      "tests/docs-wind-preview/playwright.config.mjs",
      "pnpm-lock.yaml",
      "crates/zudo-wind/catalog/zudo-wind-catalog.v1.json",
    ]) {
      expect(patterns.some((pattern) => pathMatchesDocsSite(pattern, path))).toBe(true);
    }
  });
});

describe("classifyDocsSiteChanges", () => {
  const classify = (files, complete = true) =>
    classifyDocsSiteChanges({ files, patterns, complete });

  it("skips a crate-only diff and runs when a docs change remains", () => {
    expect(classify([{ filename: "crates/zfb/src/lib.rs", status: "modified" }]).docs).toBe(false);
    expect(
      classify([
        { filename: "crates/zfb/src/lib.rs", status: "modified" },
        { filename: "docs/src/content/docs/guides/start.mdx", status: "modified" },
      ]).docs,
    ).toBe(true);
  });

  it("counts deletions and both names of a rename", () => {
    expect(classify([{ filename: "docs/old.mdx", status: "removed" }]).docs).toBe(true);
    expect(
      classify([
        {
          filename: "notes/moved.mdx",
          previous_filename: "docs/old.mdx",
          status: "renamed",
        },
      ]).docs,
    ).toBe(true);
    expect(
      classify([
        {
          filename: "docs/new.mdx",
          previous_filename: "crates/zfb/README.md",
          status: "renamed",
        },
      ]).docs,
    ).toBe(true);
  });

  it("runs for generator inputs and skips once those diffs are gone", () => {
    expect(
      classify([{ filename: "tests/wind-compatibility/inventory.v1.json", status: "modified" }])
        .docs,
    ).toBe(true);
    expect(
      classify([{ filename: "scripts/wind-compatibility/inventory.mjs", status: "modified" }]).docs,
    ).toBe(true);
    expect(classify([{ filename: "Cargo.lock", status: "modified" }]).docs).toBe(false);
  });

  it("does not report an incomplete or unusable result as no docs change", () => {
    expect(classify([{ filename: "Cargo.toml", status: "modified" }], false)).toEqual({
      docs: true,
      fallback: "incomplete",
      matched: [],
    });
    expect(classifyDocsSiteChanges({ files: [], patterns: null, complete: true })).toMatchObject({
      docs: true,
      fallback: "rules-absent",
    });
    expect(() => classifyDocsSiteChanges({ files: null, patterns, complete: true })).toThrow(
      /missing/,
    );
  });

  it("treats a complete empty diff as no docs change", () => {
    expect(classify([]).docs).toBe(false);
  });
});

describe("listPullRequestFiles", () => {
  const page = (length) =>
    Array.from({ length }, (_, index) => ({ filename: `file-${index}.txt` }));

  it("stops on a short page and rejects a failed or non-array body", async () => {
    const calls = [];
    const listed = await listPullRequestFiles({
      repo: "Takazudo/zudo-front-builder",
      pr: "1",
      token: "token",
      fetchImpl: async (url) => {
        calls.push(url);
        return response(calls.length === 1 ? page(100) : [{ filename: "docs/last.mdx" }]);
      },
    });
    expect(listed.complete).toBe(true);
    expect(listed.files).toHaveLength(101);
    await expect(
      listPullRequestFiles({
        repo: "Takazudo/zudo-front-builder",
        pr: "1",
        token: "token",
        fetchImpl: async () => response({ message: "nope" }, false, 500),
      }),
    ).rejects.toThrow(/500/);
    await expect(
      listPullRequestFiles({
        repo: "Takazudo/zudo-front-builder",
        pr: "1",
        token: "token",
        fetchImpl: async () => response({ files: [] }),
      }),
    ).rejects.toThrow(/non-array/);
  });

  it("marks the 3000-file cap incomplete", async () => {
    const listed = await listPullRequestFiles({
      repo: "Takazudo/zudo-front-builder",
      pr: "9",
      token: "token",
      fetchImpl: async () => response(page(100)),
    });
    expect(listed.complete).toBe(false);
    expect(listed.files).toHaveLength(3000);
  });
});

describe("detectDocsSiteChanges", () => {
  it("fails the detector when the files API fails and full-checks unreadable rules", async () => {
    const rulesText = readFileSync(join(rootDir, ".github/path-filters/docs-site.yml"), "utf8");
    await expect(
      detectDocsSiteChanges({
        rulesText,
        listFiles: async () => {
          throw new Error("GitHub pull files API failed: 502");
        },
      }),
    ).rejects.toThrow(/502/);
    await expect(
      detectDocsSiteChanges({
        rulesText: "docs: []\n",
        listFiles: async () => {
          throw new Error("should not list files");
        },
      }),
    ).resolves.toMatchObject({ docs: true, fallback: "rules-unreadable" });
  });
});

describe("docs-checks preview wiring", () => {
  const build = docsChecks.jobs.build;
  const gate = docsChecks.jobs["docs-gate"];
  const preview = docsChecks.jobs.preview;
  const names = build.steps.map((step) => step.name);

  it("keeps the required gate independent of preview and reuses the root snapshot", () => {
    expect(docsChecks.on.pull_request.types).toContain("ready_for_review");
    expect(gate.needs).toEqual(["changed-files", "build", "wind-previews", "docs-only-checks"]);
    expect(gate.if).toBe("${{ always() }}");
    expect(preview.needs).toEqual(["changed-files", "docs-gate"]);
    expect(preview.if).toContain("needs.docs-gate.result == 'success'");
    expect(preview.if).toContain("needs.changed-files.outputs.docs == 'true'");
    expect(preview.if).not.toMatch(/\n/);
    expect(preview.if).toContain("github.event.action == 'ready_for_review'");
    expect(preview.if).toContain("github.event.pull_request.draft == false");
    expect(preview.if).toContain(
      "github.event.pull_request.head.repo.full_name == github.repository",
    );
    expect(preview.if).toContain("github.event.pull_request.user.login != 'dependabot[bot]'");
    expect(preview.concurrency.group).toBe("pr-preview-${{ github.event.pull_request.number }}");
    expect(preview.concurrency["cancel-in-progress"]).toBe(true);
    expect(JSON.stringify(build)).not.toContain("CLOUDFLARE_");
    expect(JSON.stringify(preview)).toContain("CLOUDFLARE_API_TOKEN");
    expect(preview.permissions["pull-requests"]).toBe("write");
    expect(docsChecks.permissions.contents).toBe("read");

    const snapshot = build.steps.find(
      (step) => step.name === "Snapshot validated root docs output",
    );
    const upload = build.steps.find((step) => step.name === "Upload validated root docs output");
    const download = preview.steps.find(
      (step) => step.name === "Download validated root docs output",
    );
    expect(names.indexOf(snapshot.name)).toBeGreaterThan(
      names.indexOf("Confirm root docs previews in Chromium"),
    );
    expect(names.indexOf("Build genuine subpath docs")).toBeGreaterThan(
      names.indexOf(snapshot.name),
    );
    expect(names.indexOf(upload.name)).toBeGreaterThan(names.indexOf("Build genuine subpath docs"));
    expect(snapshot.run).toContain("cp -a docs/dist");
    expect(upload.with.name).toBe("docs-root-dist");
    expect(upload.with.path).toContain("docs-root-dist");
    expect(upload.with["include-hidden-files"]).toBe(true);
    expect(upload.with.path).not.toContain("docs/dist");
    expect(download.with.name).toBe("docs-root-dist");
    expect(preview.steps.map((step) => step.run).join("\n")).not.toContain(
      "pnpm --filter docs build",
    );
    expect(JSON.stringify(preview)).toContain("<!-- cf-workers-preview-pr -->");
    expect(JSON.stringify(preview)).toContain("missing-secret");
    expect(JSON.stringify(preview)).toContain("data.head.sha === expected");
  });

  it("full-checks when the base has no docs-site rule and has no separate preview workflow", () => {
    const detector = docsChecks.jobs["changed-files"].steps.find((step) => step.id === "docs_site");
    expect(detector.run).toContain("docs=true");
    expect(detector.run).toContain("absent-on-base");
    expect(detector.run).not.toContain("docs=false");
    expect(() =>
      readFileSync(join(rootDir, ".github/workflows/docs-pr-preview.yml"), "utf8"),
    ).toThrow();
  });
});
