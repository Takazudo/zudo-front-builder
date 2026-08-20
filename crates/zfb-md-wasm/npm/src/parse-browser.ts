import glueHref from "./wasm-parse/zfb_md_wasm_parse_glue.zfb-resource.mjs?url";
import wasmHref from "./wasm-parse/zfb_md_wasm_parse_bg.wasm?url";
import { createWasmApi } from "./runtime.js";

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

const api = createWasmApi({ glueUrl: GLUE_URL, loadWasmBytes: loadBrowserWasmBytes });

export const { init, parseToAst, version, __forceTrapForTests, __getTrapRecoveryStateForTests } =
  api;

export { MdastAdapterError, toMdastRoot } from "./mdast.js";
export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime.js";
export type {
  ParseToAstResult,
  ParseToAstOptions,
  ParseDialect,
  FrontmatterPolicy,
  ParsePipelineOptions,
  Diagnostic,
  DiagnosticSource,
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
