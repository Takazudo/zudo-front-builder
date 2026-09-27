import wasmModule from "./wasm-parse/zfb_md_wasm_parse_bg.wasm";
import { createGlue } from "./wasm-parse/zfb_md_wasm_parse_glue.zfb-factory.mjs";
import { createStaticWasmApi } from "./runtime-static.js";

const api = createStaticWasmApi({ module: wasmModule as WebAssembly.Module, createGlue });

export const { init, parseToAst, version, __forceTrapForTests, __getTrapRecoveryStateForTests } =
  api;

export { MdastAdapterError, toMdastRoot } from "./mdast.js";
export { ZfbMdWasmTrapError, ZfbMdWasmTrapRecoveryLimitError } from "./runtime-static.js";
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
