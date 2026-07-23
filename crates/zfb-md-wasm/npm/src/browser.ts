import glueHref from "./wasm/zfb_md_wasm_glue.zfb-resource.mjs?url";
import wasmHref from "./wasm/zfb_md_wasm_bg.wasm?url";
import { createWasmApi } from "./runtime.js";

// `?url` is the explicit cross-bundler asset contract: both Vite and zfb's
// pinned esbuild file loaders replace these imports with URL strings. Resolving
// them from the final entry keeps relative esbuild output correct while also
// accepting Vite's base-prefixed URLs.
const GLUE_URL = new URL(glueHref, import.meta.url);
const WASM_URL = new URL(wasmHref, import.meta.url);

async function loadBrowserWasmBytes(): Promise<ArrayBuffer> {
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
  loadWasmBytes: loadBrowserWasmBytes,
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
export { MdastAdapterError, toMdastRoot } from "./mdast.js";

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
  RawMdastData,
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
  DirectiveNodeBase,
  ContainerDirective,
  LeafDirective,
  TextDirective,
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
