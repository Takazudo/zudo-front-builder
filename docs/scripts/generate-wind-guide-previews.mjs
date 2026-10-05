#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadRecords, REPO_ROOT } from "./wind-preview-assets.mjs";
import { loadPreviewContext } from "./wind-reference-editorial.mjs";

const marker =
  /\{\/\* wind-preview: ([a-z0-9-]+)\/([a-z0-9-]+) ("(?:[^"\\]|\\.)*") \*\/\}[\s\S]*?\{\/\* wind-preview:end \*\/\}/g;
const fence = (source, language) => {
  const length = Math.max(3, ...[...source.matchAll(/`+/g)].map(([run]) => run.length + 1));
  const ticks = "`".repeat(length);
  return `${ticks}${language}\n${source.trimEnd()}\n${ticks}`;
};

/** Guide prose stays authored; only marked source/preview blocks are generated. */
export function renderGuidePreviews(source, locale, context) {
  if (!["en", "ja"].includes(locale)) throw new Error("Unsupported guide locale");
  let count = 0;
  const result = source.replace(marker, (whole, family, id, titleJson) => {
    count += 1;
    const example = context.examples.get(family)?.examples.find((item) => item.id === id);
    const asset = context.assets.get(`${family}/${id}`);
    const title = JSON.parse(titleJson);
    if (!example || !asset || !title.trim())
      throw new Error(`Missing guide example: ${family}/${id}`);
    const labels =
      locale === "ja"
        ? { config: "この例の設定", scaffold: "表示用の補助 CSS", diagnostics: "期待される診断" }
        : {
            config: "Configuration for this example",
            scaffold: "Authored demonstration CSS",
            diagnostics: "Expected diagnostics",
          };
    const preview =
      example.kind === "positive"
        ? `<WindPreviewEnhancer />\n\n<HtmlPreview html={${JSON.stringify(asset.html)}} css={${JSON.stringify(asset.css)}}${asset.head ? ` head={${JSON.stringify(asset.head)}}` : ""} title={${titleJson}} lang=${JSON.stringify(locale)} preflight={false} defaultOpen={true} showSource={true} showViewportControls={true} />`
        : `${fence(asset.html, "html")}\n\n${labels.diagnostics}: ${example.expectedDiagnostics.map(({ code, severity }) => `\`${code} (${severity})\``).join(", ")}`;
    return [
      `{/* wind-preview: ${family}/${id} ${titleJson} */}`,
      "",
      preview,
      "",
      `<Details title={${JSON.stringify(labels.config)}}>`,
      "",
      fence(JSON.stringify(asset.config, null, 2), "json"),
      "",
      "</Details>",
      "",
      `<Details title={${JSON.stringify(labels.scaffold)}}>`,
      "",
      fence(example.scaffoldCss, "css"),
      "",
      "</Details>",
      "",
      "{/* wind-preview:end */}",
    ].join("\n");
  });
  const starts = [...source.matchAll(/\{\/\* wind-preview:(?!end\b)/g)].length;
  const ends = [...source.matchAll(/\{\/\* wind-preview:end \*\/\}/g)].length;
  if (starts !== count || ends !== count) throw new Error("Malformed guide preview marker");
  return result;
}

export function generateGuidePreviews(files, check = false, root = REPO_ROOT) {
  const context = loadPreviewContext(root, loadRecords(root));
  for (const file of files) {
    const path = resolve(root, file);
    if (!/^docs\/src\/content\/docs(?:-ja)?\/zudo-wind\/[a-z0-9-]+\.mdx$/.test(file))
      throw new Error(`Expected a wind guide path: ${file}`);
    const source = readFileSync(path, "utf8");
    const rendered = renderGuidePreviews(source, file.includes("/docs-ja/") ? "ja" : "en", context);
    if (check && source !== rendered) throw new Error(`Stale guide previews: ${file}`);
    if (!check && source !== rendered) writeFileSync(path, rendered);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const check = args.includes("--check");
  const files = args.filter((arg) => arg !== "--check");
  const guides = [
    "index",
    "overview",
    "tokens",
    "utility-grammar",
    "variants",
    "configuration",
    "sources-and-candidates",
    "diagnostics-and-tools",
    "cascade-and-reset",
    "coming-from-tailwind",
  ];
  generateGuidePreviews(
    files.length
      ? files
      : ["docs", "docs-ja"].flatMap((locale) =>
          guides.map((guide) => `docs/src/content/${locale}/zudo-wind/${guide}.mdx`),
        ),
    check,
  );
}
