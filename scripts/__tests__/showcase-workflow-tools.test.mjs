import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflow = readFileSync(joinRoot(".github/workflows/node-free-smoke.yml"), "utf8");

function joinRoot(path) {
  return resolve(rootDir, path);
}

function job(name) {
  const start = workflow.indexOf(`  ${name}:\n`);
  if (start === -1) throw new Error(`missing job: ${name}`);
  const remainder = workflow.slice(start + 1);
  const next = remainder.search(/^  [a-zA-Z0-9_-]+:\n/m);
  return workflow.slice(start, next === -1 ? undefined : start + 1 + next);
}

function step(jobText, name) {
  const marker = `      - name: ${name}\n`;
  const start = jobText.indexOf(marker);
  if (start === -1) throw new Error(`missing step: ${name}`);
  const remainder = jobText.slice(start + marker.length);
  const next = remainder.search(/^      - /m);
  return jobText.slice(start, next === -1 ? undefined : start + marker.length + next);
}

describe("showcase workflow tool provenance", () => {
  it("evaluates the preview guard for draft, fork, bot and trusted ready events", () => {
    const preview = job("showcase-preview");
    const expression = preview.match(/^    if: >-\n((?:      .+\n)+)/m)[1];
    const allowed = Function("github", "return (" + expression + ")");
    const github = {
      event_name: "pull_request",
      repository: "owner/repo",
      actor: "owner",
      event: { pull_request: { draft: true, head: { repo: { full_name: "owner/repo" } } } },
    };
    expect(allowed(github)).toBe(false);
    github.event.pull_request.draft = false;
    expect(allowed(github)).toBe(true);
    github.event.pull_request.head.repo.full_name = "fork/repo";
    expect(allowed(github)).toBe(false);
    github.event.pull_request.head.repo.full_name = "owner/repo";
    github.actor = "dependabot[bot]";
    expect(allowed(github)).toBe(false);
    github.event_name = "push";
    expect(allowed(github)).toBe(false);
  });

  it("keeps draft filtering out of triggers and verification jobs", () => {
    expect(workflow.slice(0, workflow.indexOf("\njobs:\n"))).not.toContain("pull_request.draft");
    for (const name of [
      "build-binary",
      "scaffold-e2e",
      "built-site-browser-smoke",
      "smoke-amd64",
      "smoke-amd64-al2023",
      "smoke-arm64",
      "smoke-amd64-ts",
      "smoke-arm64-ts",
    ]) {
      expect(job(name)).not.toContain("pull_request.draft");
    }
  });

  for (const jobName of ["showcase-deploy", "showcase-preview"]) {
    it(`${jobName} installs and executes only root lockfile tools`, () => {
      const jobText = job(jobName);
      const beforeSteps = jobText.slice(0, jobText.indexOf("    steps:\n"));
      const install = step(jobText, "Install showcase tools");
      const validate = step(jobText, "Validate emitted HTML (html-validate)");

      expect(beforeSteps).not.toMatch(/^    env:/m);
      expect(jobText).toContain(
        "uses: pnpm/action-setup@ea17c68df8912ef543352723c149a84f56e3d413 # v6.1.0",
      );
      expect(install).toContain("pnpm install --frozen-lockfile --filter zudo-front-builder");
      expect(validate).toContain("working-directory: create-zfb-showcase");
      expect(validate).toContain('../node_modules/.bin/html-validate "dist/**/*.html"');
      expect(jobText).not.toMatch(/\bnpx\b/);
    });
  }

  it("keeps preview credentials on the Wrangler upload step only", () => {
    const preview = job("showcase-preview");
    const upload = step(preview, "Upload preview version");

    expect(
      preview.slice(0, preview.indexOf("      - name: Upload preview version\n")),
    ).not.toContain("CLOUDFLARE_API_TOKEN");
    expect(upload).toContain("working-directory: create-zfb-showcase");
    expect(upload).toContain("CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}");
    expect(upload).toContain("CLOUDFLARE_ACCOUNT_ID: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}");
    expect(upload).toContain("../node_modules/.bin/wrangler versions upload");
  });

  it("keeps production validation before banner injection and deploys from the showcase cwd", () => {
    const production = job("showcase-deploy");
    const validate = step(production, "Validate emitted HTML (html-validate)");
    const banner = step(production, "Inject the showcase banner");
    const deploy = step(production, "Deploy worker");

    expect(production.indexOf("Validate emitted HTML")).toBeLessThan(
      production.indexOf("Inject the showcase banner"),
    );
    expect(validate).not.toContain("CLOUDFLARE_");
    expect(banner).not.toContain("CLOUDFLARE_");
    expect(deploy).toContain("working-directory: create-zfb-showcase");
    expect(deploy).toContain("../node_modules/.bin/wrangler deploy");
  });

  it("skips draft PRs in the preview job only", () => {
    const preview = job("showcase-preview");
    const condition = preview.slice(
      preview.indexOf("    if: >-\n"),
      preview.indexOf("    runs-on:"),
    );

    expect(condition).toBe(
      "    if: >-\n" +
        "      github.event_name == 'pull_request' &&\n" +
        "      github.event.pull_request.head.repo.full_name == github.repository &&\n" +
        "      github.actor != 'dependabot[bot]' &&\n" +
        "      !github.event.pull_request.draft\n",
    );
    expect(job("showcase-deploy")).not.toContain("pull_request.draft");
    expect(job("showcase-deploy")).toContain("github.ref == 'refs/heads/main'");
    expect(job("scaffold-e2e")).not.toContain("pull_request.draft");
  });

  it("declares no types on the pull_request trigger", () => {
    expect(workflow).toMatch(/^on:\n {2}pull_request:\n {2}push:\n/m);
  });
});
