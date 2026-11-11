import type { PlaygroundSample } from "./sample-picker";

export type PlaygroundLocale = "en" | "ja";

export interface PlaygroundProps {
  locale?: PlaygroundLocale;
  /** Resolved by the server wrapper and transported with the island props. */
  basePath?: string;
}

const en = {
  input: "Input",
  output: "Output",
  run: "Run",
  pending: "Running…",
  firstRunNotice: "The first Run downloads a WebAssembly module.",
  apiReference: "API reference",
  samples: "Samples",
  noDiagnostics: "No diagnostics.",
  diagnostics: "Diagnostics",
  renderDiagnostics: "Render diagnostics",
  parseDiagnostics: "Parse diagnostics",
  highlightDiagnostics: "Highlight diagnostics",
  htmlOutput: "HTML output",
  htmlSource: "HTML source",
  htmlPreview: "HTML preview",
  preview: "Preview",
  renderedHtmlPreview: "Rendered HTML preview",
  markdownSource: "Markdown or MDX source",
  mdxSource: "MDX source",
  sourceCode: "Source code",
  filenameHelp: "Use a filename ending in .md or .mdx.",
  filenameError: "filename must end in .md or .mdx.",
  builtInDefault: "built-in default",
  compilePrompt: "Press Run to emit an ES module.",
  noModule: "No module source was emitted.",
  parsePrompt: "Run to inspect the raw mdast tree.",
  nodeCount: "node count",
  maxDepth: "max depth",
  highlightPrompt: "Run the highlighter to see semantic HTML.",
  optionsRejected: "No HTML output: the supplied options were rejected.",
  noHtml: "No HTML output was returned.",
  semanticHtml: "Rendered semantic HTML",
  rawHtml: "Raw HTML source",
  roleHelp:
    "Optional full-role class overrides; clear a field to restore its default semantic class.",
  keywordDefault: "default: hi-kw",
  stringDefault: "default: hi-str",
  success: "Success",
  warning: "Success with warning",
  optionsError: "Options error",
  highlightError: "Highlight error",
  error: "error",
  diagnosticWarning: "warning",
  runError: "Execution failed",
};

export type PlaygroundCopy = typeof en;

const ja: PlaygroundCopy = {
  input: "入力",
  output: "出力",
  run: "実行",
  pending: "実行中…",
  firstRunNotice: "初回の実行時に WebAssembly モジュールをダウンロードします。",
  apiReference: "API リファレンス",
  samples: "サンプル",
  noDiagnostics: "診断はありません。",
  diagnostics: "診断",
  renderDiagnostics: "レンダリングの診断",
  parseDiagnostics: "解析の診断",
  highlightDiagnostics: "ハイライトの診断",
  htmlOutput: "HTML 出力",
  htmlSource: "HTML ソース",
  htmlPreview: "HTML プレビュー",
  preview: "プレビュー",
  renderedHtmlPreview: "レンダリングされた HTML のプレビュー",
  markdownSource: "Markdown または MDX のソース",
  mdxSource: "MDX ソース",
  sourceCode: "ソースコード",
  filenameHelp: "末尾が .md または .mdx のファイル名を指定してください。",
  filenameError: "filename の末尾は .md または .mdx にしてください。",
  builtInDefault: "組み込みの既定値",
  compilePrompt: "実行すると ES モジュールを出力します。",
  noModule: "モジュールのソースは出力されませんでした。",
  parsePrompt: "実行すると raw mdast ツリーを確認できます。",
  nodeCount: "ノード数",
  maxDepth: "最大の深さ",
  highlightPrompt: "実行すると意味的な HTML を確認できます。",
  optionsRejected: "HTML 出力はありません。指定したオプションが拒否されました。",
  noHtml: "HTML 出力は返されませんでした。",
  semanticHtml: "レンダリングされた意味的 HTML",
  rawHtml: "raw HTML ソース",
  roleHelp: "各ロールのクラスを任意で上書きできます。入力を空にすると既定の意味クラスに戻ります。",
  keywordDefault: "既定値: hi-kw",
  stringDefault: "既定値: hi-str",
  success: "成功",
  warning: "成功（警告あり）",
  optionsError: "オプションエラー",
  highlightError: "ハイライトエラー",
  error: "エラー",
  diagnosticWarning: "警告",
  runError: "実行に失敗しました",
};

export function playgroundCopy(locale: PlaygroundLocale = "en"): PlaygroundCopy {
  return locale === "ja" ? ja : en;
}

const sampleLabels: Record<string, string> = {
  "render-basics": "GFM と HTML",
  "admonition-directive": "注記ディレクティブ",
  "compile-sample": "MDX サンプル",
  "raw-ast": "未加工の MDX",
  incomplete: "未完成の HTML",
  "unknown-language": "未知の言語",
};

/** Only presentation labels change; selection IDs and executable samples stay intact. */
export function localizedSamples<T extends PlaygroundSample>(
  samples: readonly T[],
  locale: PlaygroundLocale = "en",
): readonly T[] {
  return locale === "ja"
    ? samples.map((sample) => ({ ...sample, label: sampleLabels[sample.id] ?? sample.label }))
    : samples;
}

export function playgroundApiHref(locale: PlaygroundLocale = "en", basePath = ""): string {
  return `${basePath.replace(/\/$/, "")}/${locale === "ja" ? "ja/" : ""}docs/api/md-wasm/`;
}
