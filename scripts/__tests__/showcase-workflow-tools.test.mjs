import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

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
  for (const jobName of ["showcase-deploy", "showcase-preview"]) {
    it(`${jobName} installs and executes only root lockfile tools`, () => {
      const jobText = job(jobName);
      const beforeSteps = jobText.slice(0, jobText.indexOf("    steps:\n"));
      const install = step(jobText, "Install showcase tools");
      const validate = step(jobText, "Validate emitted HTML (html-validate)");

      expect(beforeSteps).not.toMatch(/^    env:/m);
      expect(jobText).toContain(
        "uses: pnpm/action-setup@0e279bb959325dab635dd2c09392533439d90093 # v6.0.8",
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
});
