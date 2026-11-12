import { expect, test } from "@playwright/test";

const base = process.env.WIND_DOCS_BASE ?? "/";
const tools = [
  {
    slug: "render",
    target: "RenderPlayground",
    alternate: ["Admonition directive", "注記ディレクティブ"],
  },
  {
    slug: "compile",
    target: "CompilePlayground",
    alternate: ["Admonition directive", "注記ディレクティブ"],
  },
  {
    slug: "parse",
    target: "ParsePlayground",
    alternate: ["Admonition directive", "注記ディレクティブ"],
  },
  {
    slug: "highlight",
    target: "HighlightPlayground",
    alternate: ["Unknown language", "未知の言語"],
  },
];
const copy = {
  en: {
    run: "Run",
    pending: "Running…",
    failure: "Execution failed",
    api: "API reference",
    noDiagnostics: "No diagnostics.",
  },
  ja: {
    run: "実行",
    pending: "実行中…",
    failure: "実行に失敗しました",
    api: "API リファレンス",
    noDiagnostics: "診断はありません。",
  },
};

for (const [localeIndex, locale] of ["en", "ja"].entries()) {
  for (const tool of tools) {
    test(`${locale} ${tool.slug}: serialized locale, samples, failure/retry, pending, editing and navigation`, async ({
      page,
    }) => {
      const labels = copy[locale];
      const prefix = `${base}${locale === "ja" ? "ja/" : ""}docs/`;
      const route = `${prefix}playground/${tool.slug}/`;
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto(route);
      const island = page.locator(`[data-zfb-island="${tool.target}"]`);
      await island.scrollIntoViewIfNeeded();
      await expect(island).toBeVisible();
      await expect(island).toHaveAttribute("data-zfb-island-mounted", "");
      const props = JSON.parse(await island.getAttribute("data-props"));
      expect(props.locale).toBe(locale);
      expect(props.basePath).toBe(base);
      await expect(island.getByRole("link", { name: labels.api, exact: true })).toHaveAttribute(
        "href",
        `${prefix}api/md-wasm/`,
      );
      if (locale === "ja")
        await island.screenshot({ path: test.info().outputPath(`ja-${tool.slug}-controls.png`) });
      const alternate = island.getByRole("button", {
        name: tool.alternate[localeIndex],
        exact: true,
      });
      await alternate.click();
      await expect(alternate).toHaveAttribute("aria-pressed", "true");
      if (tool.slug === "highlight")
        await island.getByRole("button", { name: "JavaScript", exact: true }).click();
      const input = island.locator("textarea");
      const marker = `auditedLocale${locale}${tool.slug}`;
      await input.fill(
        tool.slug === "highlight"
          ? `const ${marker} = "ready";`
          : `# ${marker}\n\nLocalized interface audit.`,
      );
      await expect(alternate).toHaveAttribute("aria-pressed", "false");
      const run = island.locator("button[aria-busy]");

      // Exercise a normal transport rejection; engine diagnostics remain verbatim.
      await page.route("**/*.wasm", (route) => route.abort("failed"));
      await run.click();
      await expect(island).toContainText(labels.failure);
      await page.unroute("**/*.wasm");

      let release;
      const gate = new Promise((resolve) => {
        release = resolve;
      });
      await page.route("**/*.wasm", async (route) => {
        await gate;
        await route.continue();
      });
      await run.click();
      try {
        await expect(run).toHaveAttribute("aria-busy", "true");
        await expect(run).toHaveText(labels.pending);
      } finally {
        release();
      }
      await expect(run).toHaveAttribute("aria-busy", "false");
      await expect(run).toHaveText(labels.run);
      await expect(island.locator("pre").filter({ hasText: marker }).first()).toBeVisible();
      await expect(island).toContainText(labels.noDiagnostics);
      if (tool.slug === "compile")
        await expect(island).toContainText("@takazudo/zfb/zudo-react/jsx-runtime");

      if (tool.slug === "highlight") {
        await island.getByLabel("language", { exact: true }).fill("");
        await run.click();
        await expect(island).toContainText(locale === "ja" ? "オプションエラー" : "Options error");
        await expect(island).toContainText(
          locale === "ja"
            ? "HTML 出力はありません。指定したオプションが拒否されました。"
            : "No HTML output: the supplied options were rejected.",
        );
      }
      if (tool.slug === "compile") {
        await island.getByLabel("filename", { exact: false }).fill("invalid.txt");
        await run.click();
        await expect(island).toContainText(
          locale === "ja"
            ? "filename の末尾は .md または .mdx にしてください。"
            : "filename must end in .md or .mdx.",
        );
      }

      // Real same-locale client navigation and browser Back must transport locale again.
      await page.evaluate((value) => {
        window.__docsAuditSentinel = value;
      }, marker);
      await page.locator(`article a[href="${prefix}api/md-wasm/"]`).first().click();
      await expect(page).toHaveURL(new RegExp(`${prefix}api/md-wasm/?$`));
      expect(await page.evaluate(() => window.__docsAuditSentinel)).toBe(marker);
      await page.goBack();
      await expect(page).toHaveURL(new RegExp(`${route}?$`));
      expect(await page.evaluate(() => window.__docsAuditSentinel)).toBe(marker);
      const returned = page.locator(`[data-zfb-island="${tool.target}"]`);
      await returned.scrollIntoViewIfNeeded();
      await expect(returned).toHaveAttribute("data-zfb-island-mounted", "");
      await expect(returned.locator("button[aria-busy]")).toHaveText(labels.run);
      await returned
        .getByRole("button", { name: tool.alternate[localeIndex], exact: true })
        .click();
      await expect(
        returned.getByRole("button", { name: tool.alternate[localeIndex], exact: true }),
      ).toHaveAttribute("aria-pressed", "true");
      expect(JSON.parse(await returned.getAttribute("data-props")).locale).toBe(locale);
      if (tool.slug === "highlight") {
        await returned.locator("button[aria-busy]").click();
        await expect(returned).toContainText(
          locale === "ja" ? "成功（警告あり）" : "Success with warning",
        );
        await expect(returned).toContainText("not-a-bundled-syntax");
        await returned.getByRole("button", { name: "JavaScript", exact: true }).click();
      }
      if (tool.slug === "compile")
        await returned.getByLabel("filename", { exact: false }).fill("audit.mdx");
      const returnMarker = `${marker}AfterBack`;
      await returned
        .locator("textarea")
        .fill(tool.slug === "highlight" ? `const ${returnMarker} = "ready";` : `# ${returnMarker}`);
      await returned.locator("button[aria-busy]").click();
      await expect(returned.locator("button[aria-busy]")).toHaveAttribute("aria-busy", "false");
      await expect(returned.locator("pre").filter({ hasText: returnMarker }).first()).toBeVisible();
      await expect(returned).toContainText(labels.noDiagnostics);
      expect(errors).toEqual([]);
    });
  }
}
