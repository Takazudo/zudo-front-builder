import { createWasmApi } from "./runtime.js";

const GLUE_URL = new URL("./wasm/zfb_md_wasm_glue.zfb-resource.mjs", import.meta.url);
const WASM_URL = new URL("./wasm/zfb_md_wasm_bg.wasm", import.meta.url);

function isNode(): boolean {
  return typeof process !== "undefined" && !!process.versions?.node;
}

// Node's fetch cannot load file: URLs, while direct browser/module consumers
// should keep fetching the files relative to this package entry. Browser-aware
// bundlers select browser.ts instead, which has static resource URL edges.
async function loadDirectWasmBytes(): Promise<ArrayBuffer> {
  if (isNode()) {
    const [{ readFile }, { fileURLToPath }] = await Promise.all([
      import("node:fs/promises"),
      import("node:url"),
    ]);
    const bytes = await readFile(fileURLToPath(WASM_URL));
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
  }

  const response = await fetch(WASM_URL);
  if (!response.ok) {
    throw new Error(
      `zfb-md-wasm: failed to fetch wasm binary: ${response.status} ${response.statusText}`,
    );
  }
  return response.arrayBuffer();
}

const api = createWasmApi({
  glueUrl: GLUE_URL,
  loadWasmBytes: loadDirectWasmBytes,
});

export const {
  init,
  compile,
  renderHtml,
  // Raw-mdast export (zfb#1857, epic zfb#1854) -- see types.ts's
  // ParseToAstResult/MdastNode docs for the result shape and contract.
  parseToAst,
  highlightCode,
  version,
  __forceTrapForTests,
  __getTrapRecoveryStateForTests,
} = api;

export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime.js";

export type {
  CompileResult,
  RenderHtmlResult,
  ParseToAstResult,
  ParseToAstOptions,
  ParseDialect,
  FrontmatterPolicy,
  ParsePipelineOptions,
  Diagnostic,
  DiagnosticSource,
  ZfbMdWasmOptions,
  PipelineOptions,
  GfmOptions,
  CodeHighlightMode,
  CodeHighlightOptions,
  MarkdownFeaturesConfig,
  JsxRuntime,
  HighlightRole,
  HighlightCodeOptions,
  HighlightCodeResult,
  HighlightDiagnostic,
  HighlightDiagnosticSource,
  AstPoint,
  AstPosition,
  MarkdownRsStop,
  MdastNode,
  MdastRoot,
  UnknownMdastNode,
  Root,
  Paragraph,
  Heading,
  ThematicBreak,
  Blockquote,
  List,
  ListItem,
  Html,
  Code,
  Definition,
  Text,
  Emphasis,
  Strong,
  InlineCode,
  Break,
  Link,
  Image,
  ReferenceKind,
  LinkReference,
  ImageReference,
  FootnoteDefinition,
  FootnoteReference,
  TableAlign,
  Table,
  TableRow,
  TableCell,
  Delete,
  Yaml,
  MdxFlowExpression,
  MdxTextExpression,
  MdxJsxFlowElement,
  MdxJsxTextElement,
  MdxJsxAttributeContent,
  MdxJsxAttribute,
  MdxJsxAttributeValueExpression,
  MdxJsxExpressionAttribute,
} from "./types.js";
