// Empty type re-exports are the declaration-emitting form of augmentation
// imports: TypeScript elides `import type {}` from `.d.ts` output, while these
// remain in the published declaration and load both content-model registries.
export type {} from "mdast-util-directive";
export type {} from "mdast-util-mdx";
import type {
  BlockContent,
  DefinitionContent,
  ListItem,
  Paragraph,
  PhrasingContent,
  Root as EcosystemRoot,
  RootContent,
  TableCell,
  TableRow,
  TopLevelContent,
} from "mdast";
import type {
  MdxJsxAttribute,
  MdxJsxAttributeValueExpression,
  MdxJsxExpressionAttribute,
} from "mdast-util-mdx";

import type { MdastRoot } from "./types.js";

type RecordValue = Record<string, unknown>;
type FlowContent = BlockContent | DefinitionContent;
type Position = NonNullable<EcosystemRoot["position"]>;
type Point = Position["start"];
type ValidPoint = Point & { line: number; column: number; offset: number };

const PHRASING_TYPES = new Set([
  "break",
  "delete",
  "emphasis",
  "footnoteReference",
  "html",
  "image",
  "imageReference",
  "inlineCode",
  "link",
  "linkReference",
  "mdxJsxTextElement",
  "mdxTextExpression",
  "strong",
  "text",
  "textDirective",
]);

const FLOW_TYPES = new Set([
  "blockquote",
  "code",
  "containerDirective",
  "definition",
  "footnoteDefinition",
  "heading",
  "html",
  "leafDirective",
  "list",
  "mdxFlowExpression",
  "mdxJsxFlowElement",
  "paragraph",
  "table",
  "thematicBreak",
]);

const TOP_LEVEL_TYPES = new Set([...FLOW_TYPES, "yaml"]);

/** A deterministic validation failure raised by {@link toMdastRoot}. */
export class MdastAdapterError extends TypeError {
  readonly path: string;
  readonly nodeType: string | null;

  constructor(path: string, nodeType: string | null, detail: string) {
    const displayType = nodeType === null ? "null" : JSON.stringify(nodeType);
    super(`Invalid mdast node at ${path} (type ${displayType}): ${detail}`);
    this.name = "MdastAdapterError";
    this.path = path;
    this.nodeType = nodeType;
  }
}

function fail(path: string, nodeType: string | null, detail: string): never {
  throw new MdastAdapterError(path, nodeType, detail);
}

function isRecord(value: unknown): value is RecordValue {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isPlainRecord(value: unknown): value is RecordValue {
  if (!isRecord(value)) return false;
  const prototype: unknown = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function observedType(value: unknown): string | null {
  return isRecord(value) && typeof value.type === "string" ? value.type : null;
}

function assertOnlyKeys(
  value: RecordValue,
  allowed: readonly string[],
  path: string,
  nodeType: string | null,
): void {
  const allowedSet = new Set(allowed);
  for (const key of Object.keys(value)) {
    if (!allowedSet.has(key)) {
      fail(`${path}.${key}`, nodeType, `unexpected field ${JSON.stringify(key)}`);
    }
  }
}

function requiredString(
  value: RecordValue,
  key: string,
  path: string,
  nodeType: string | null,
): string {
  const field = value[key];
  if (typeof field !== "string") fail(`${path}.${key}`, nodeType, "expected a string");
  return field;
}

function optionalString(
  value: RecordValue,
  key: string,
  path: string,
  nodeType: string | null,
): string | undefined {
  if (!(key in value)) return undefined;
  return requiredString(value, key, path, nodeType);
}

function requiredBoolean(
  value: RecordValue,
  key: string,
  path: string,
  nodeType: string | null,
): boolean {
  const field = value[key];
  if (typeof field !== "boolean") fail(`${path}.${key}`, nodeType, "expected a boolean");
  return field;
}

function optionalBoolean(
  value: RecordValue,
  key: string,
  path: string,
  nodeType: string | null,
): boolean | undefined {
  if (!(key in value)) return undefined;
  return requiredBoolean(value, key, path, nodeType);
}

function integer(value: unknown, path: string, nodeType: string | null, minimum: number): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < minimum) {
    fail(path, nodeType, `expected an integer greater than or equal to ${minimum}`);
  }
  return value;
}

function point(value: unknown, path: string, nodeType: string | null): ValidPoint {
  if (!isRecord(value)) fail(path, nodeType, "expected a point object");
  assertOnlyKeys(value, ["line", "column", "offset"], path, nodeType);
  return {
    line: integer(value.line, `${path}.line`, nodeType, 1),
    column: integer(value.column, `${path}.column`, nodeType, 1),
    offset: integer(value.offset, `${path}.offset`, nodeType, 0),
  };
}

function position(value: RecordValue, path: string, nodeType: string): Position {
  const raw = value.position;
  if (!isRecord(raw)) fail(`${path}.position`, nodeType, "expected a position object");
  assertOnlyKeys(raw, ["start", "end"], `${path}.position`, nodeType);
  const start = point(raw.start, `${path}.position.start`, nodeType);
  const end = point(raw.end, `${path}.position.end`, nodeType);
  if (
    end.offset < start.offset ||
    end.line < start.line ||
    (end.line === start.line && end.column < start.column)
  ) {
    fail(`${path}.position.end`, nodeType, "position end precedes position start");
  }
  return { start, end };
}

function cloneDataValue(
  value: unknown,
  path: string,
  nodeType: string,
  ancestors: Set<object>,
): unknown {
  if (
    value === null ||
    value === undefined ||
    typeof value === "string" ||
    typeof value === "boolean"
  ) {
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) fail(path, nodeType, "data contains a non-finite number");
    return value;
  }
  if (typeof value !== "object") fail(path, nodeType, "data must contain cloneable values");
  if (ancestors.has(value)) fail(path, nodeType, "data must not contain cycles");
  ancestors.add(value);
  let cloned: unknown;
  if (Array.isArray(value)) {
    cloned = value.map((item, index) =>
      cloneDataValue(item, `${path}[${index}]`, nodeType, ancestors),
    );
  } else {
    if (!isPlainRecord(value)) fail(path, nodeType, "data must contain only plain objects");
    cloned = Object.fromEntries(
      Object.entries(value).map(([key, item]) => [
        key,
        cloneDataValue(item, `${path}.${key}`, nodeType, ancestors),
      ]),
    );
  }
  ancestors.delete(value);
  return cloned;
}

function data(value: RecordValue, path: string, nodeType: string): RecordValue | undefined {
  if (!("data" in value)) return undefined;
  if (!isPlainRecord(value.data)) {
    fail(`${path}.data`, nodeType, "expected a non-null plain object");
  }
  const cloned = cloneDataValue(value.data, `${path}.data`, nodeType, new Set());
  if (!isRecord(cloned)) fail(`${path}.data`, nodeType, "expected a data object");
  return cloned;
}

function base(value: RecordValue, path: string, nodeType: string) {
  const nodeData = data(value, path, nodeType);
  return {
    position: position(value, path, nodeType),
    ...(nodeData === undefined ? {} : { data: nodeData }),
  };
}

function array(value: unknown, path: string, nodeType: string): unknown[] {
  if (!Array.isArray(value)) fail(path, nodeType, "expected an array");
  return value;
}

function stops(value: unknown, path: string, nodeType: string): void {
  for (const [index, pair] of array(value, path, nodeType).entries()) {
    const pairPath = `${path}[${index}]`;
    if (!Array.isArray(pair) || pair.length !== 2) {
      fail(pairPath, nodeType, "expected a two-integer markdown-rs stop pair");
    }
    integer(pair[0], `${pairPath}[0]`, nodeType, 0);
    integer(pair[1], `${pairPath}[1]`, nodeType, 0);
  }
}

function attributes(value: unknown, path: string, nodeType: string) {
  return array(value, path, nodeType).map((attribute, index) =>
    attributeRecord(attribute, `${path}[${index}]`, nodeType),
  );
}

function attributeRecord(
  value: unknown,
  path: string,
  parentType: string,
): MdxJsxAttribute | MdxJsxExpressionAttribute {
  if (!isRecord(value)) fail(path, parentType, "expected an MDX JSX attribute record");
  const type = typeof value.type === "string" ? value.type : null;
  if (type === "mdxJsxAttribute") {
    assertOnlyKeys(value, ["type", "data", "name", "value"], path, type);
    const name = requiredString(value, "name", path, type);
    const nodeData = data(value, path, type);
    if (!("value" in value)) {
      return { type, ...(nodeData === undefined ? {} : { data: nodeData }), name };
    }
    if (typeof value.value === "string") {
      return {
        type,
        ...(nodeData === undefined ? {} : { data: nodeData }),
        name,
        value: value.value,
      };
    }
    return {
      type,
      ...(nodeData === undefined ? {} : { data: nodeData }),
      name,
      value: attributeValue(value.value, `${path}.value`),
    };
  }
  if (type === "mdxJsxExpressionAttribute") {
    assertOnlyKeys(value, ["type", "data", "value", "_markdownRsStops"], path, type);
    const expression = requiredString(value, "value", path, type);
    stops(value._markdownRsStops, `${path}._markdownRsStops`, type);
    const nodeData = data(value, path, type);
    return { type, ...(nodeData === undefined ? {} : { data: nodeData }), value: expression };
  }
  fail(path, type, "unsupported MDX JSX attribute type");
}

function attributeValue(value: unknown, path: string): MdxJsxAttributeValueExpression {
  const type = observedType(value);
  if (!isRecord(value) || type !== "mdxJsxAttributeValueExpression") {
    fail(path, type, "expected an MDX JSX attribute value expression");
  }
  assertOnlyKeys(value, ["type", "data", "value", "_markdownRsStops"], path, type);
  const expression = requiredString(value, "value", path, type);
  stops(value._markdownRsStops, `${path}._markdownRsStops`, type);
  const nodeData = data(value, path, type);
  return { type, ...(nodeData === undefined ? {} : { data: nodeData }), value: expression };
}

function directiveAttributes(
  value: unknown,
  path: string,
  nodeType: string,
): Record<string, string> {
  if (!isPlainRecord(value)) fail(path, nodeType, "expected a plain attribute object");
  const entries: [string, string][] = [];
  for (const [key, item] of Object.entries(value)) {
    if (typeof item !== "string") fail(`${path}.${key}`, nodeType, "expected a string");
    entries.push([key, item]);
  }
  return Object.fromEntries(entries);
}

function children<T>(
  value: unknown,
  path: string,
  nodeType: string,
  adapt: (child: unknown, childPath: string) => T,
): T[] {
  return array(value, path, nodeType).map((child, index) => adapt(child, `${path}[${index}]`));
}

function adaptPhrasing(value: unknown, path: string): PhrasingContent {
  const type = observedType(value);
  if (type === null || !PHRASING_TYPES.has(type)) {
    fail(path, type, "node is not valid phrasing content");
  }
  const node = adaptContent(value, path);
  if (!isPhrasing(node)) fail(path, type, "node is not valid phrasing content");
  return node;
}

function adaptFlow(value: unknown, path: string): FlowContent {
  const type = observedType(value);
  if (type === null || !FLOW_TYPES.has(type)) fail(path, type, "node is not valid flow content");
  const node = adaptContent(value, path);
  if (!isFlow(node)) fail(path, type, "node is not valid flow content");
  return node;
}

function adaptTopLevel(value: unknown, path: string): TopLevelContent {
  const type = observedType(value);
  if (type === null || !TOP_LEVEL_TYPES.has(type)) {
    fail(path, type, "node is not valid top-level content");
  }
  const node = adaptContent(value, path);
  if (!isTopLevel(node)) fail(path, type, "node is not valid top-level content");
  return node;
}

function isPhrasing(node: RootContent): node is PhrasingContent {
  return PHRASING_TYPES.has(node.type);
}

function isFlow(node: RootContent): node is FlowContent {
  return FLOW_TYPES.has(node.type);
}

function isTopLevel(node: RootContent): node is TopLevelContent {
  return TOP_LEVEL_TYPES.has(node.type);
}

function adaptContent(value: unknown, path: string): RootContent {
  const type = observedType(value);
  if (!isRecord(value) || type === null) fail(path, type, "expected an object with a string type");
  const nodeBase = base(value, path, type);

  switch (type) {
    case "paragraph": {
      assertOnlyKeys(value, ["type", "position", "data", "children"], path, type);
      const node: Paragraph = {
        type,
        ...nodeBase,
        children: children(value.children, `${path}.children`, type, adaptPhrasing),
      };
      return node;
    }
    case "heading": {
      assertOnlyKeys(value, ["type", "position", "data", "depth", "children"], path, type);
      const depth = integer(value.depth, `${path}.depth`, type, 1);
      if (depth > 6) fail(`${path}.depth`, type, "expected an integer from 1 through 6");
      const headingDepth =
        depth === 1 ? 1 : depth === 2 ? 2 : depth === 3 ? 3 : depth === 4 ? 4 : depth === 5 ? 5 : 6;
      return {
        type,
        ...nodeBase,
        depth: headingDepth,
        children: children(value.children, `${path}.children`, type, adaptPhrasing),
      };
    }
    case "blockquote":
    case "footnoteDefinition": {
      const association = type === "footnoteDefinition";
      assertOnlyKeys(
        value,
        association
          ? ["type", "position", "data", "identifier", "label", "children"]
          : ["type", "position", "data", "children"],
        path,
        type,
      );
      const childNodes = children(value.children, `${path}.children`, type, adaptFlow);
      if (type === "blockquote") return { type, ...nodeBase, children: childNodes };
      const identifier = requiredString(value, "identifier", path, type);
      const label = optionalString(value, "label", path, type);
      return {
        type,
        ...nodeBase,
        identifier,
        ...(label === undefined ? {} : { label }),
        children: childNodes,
      };
    }
    case "list": {
      assertOnlyKeys(
        value,
        ["type", "position", "data", "ordered", "start", "spread", "children"],
        path,
        type,
      );
      const ordered = requiredBoolean(value, "ordered", path, type);
      const spread = requiredBoolean(value, "spread", path, type);
      let start: number | undefined;
      if ("start" in value) start = integer(value.start, `${path}.start`, type, 0);
      if (!ordered && start !== undefined)
        fail(`${path}.start`, type, "unordered lists must omit start");
      return {
        type,
        ...nodeBase,
        ordered,
        ...(start === undefined ? {} : { start }),
        spread,
        children: children(value.children, `${path}.children`, type, (child, childPath) => {
          const adapted = adaptContent(child, childPath);
          if (adapted.type !== "listItem")
            fail(childPath, adapted.type, "list children must be listItem nodes");
          return adapted;
        }),
      };
    }
    case "listItem": {
      assertOnlyKeys(
        value,
        ["type", "position", "data", "spread", "checked", "children"],
        path,
        type,
      );
      const checked = optionalBoolean(value, "checked", path, type);
      const node: ListItem = {
        type,
        ...nodeBase,
        spread: requiredBoolean(value, "spread", path, type),
        ...(checked === undefined ? {} : { checked }),
        children: children(value.children, `${path}.children`, type, adaptFlow),
      };
      return node;
    }
    case "table": {
      assertOnlyKeys(value, ["type", "position", "data", "align", "children"], path, type);
      const align = array(value.align, `${path}.align`, type).map((item, index) => {
        if (item !== null && item !== "left" && item !== "right" && item !== "center") {
          fail(`${path}.align[${index}]`, type, "expected left, right, center, or null");
        }
        return item;
      });
      return {
        type,
        ...nodeBase,
        align,
        children: children(value.children, `${path}.children`, type, (child, childPath) => {
          const adapted = adaptContent(child, childPath);
          if (adapted.type !== "tableRow")
            fail(childPath, adapted.type, "table children must be tableRow nodes");
          return adapted;
        }),
      };
    }
    case "tableRow": {
      assertOnlyKeys(value, ["type", "position", "data", "children"], path, type);
      const row: TableRow = {
        type,
        ...nodeBase,
        children: children(value.children, `${path}.children`, type, (child, childPath) => {
          const adapted = adaptContent(child, childPath);
          if (adapted.type !== "tableCell")
            fail(childPath, adapted.type, "table row children must be tableCell nodes");
          return adapted;
        }),
      };
      return row;
    }
    case "tableCell": {
      assertOnlyKeys(value, ["type", "position", "data", "children"], path, type);
      const cell: TableCell = {
        type,
        ...nodeBase,
        children: children(value.children, `${path}.children`, type, adaptPhrasing),
      };
      return cell;
    }
    case "emphasis":
    case "strong":
    case "delete":
    case "link":
    case "linkReference": {
      const resource = type === "link";
      const reference = type === "linkReference";
      const allowed = ["type", "position", "data", "children"];
      if (resource) allowed.push("url", "title");
      if (reference) allowed.push("referenceType", "identifier", "label");
      assertOnlyKeys(value, allowed, path, type);
      const childNodes = children(value.children, `${path}.children`, type, adaptPhrasing);
      if (type === "emphasis" || type === "strong" || type === "delete") {
        return { type, ...nodeBase, children: childNodes };
      }
      if (type === "link") {
        const title = optionalString(value, "title", path, type);
        return {
          type,
          ...nodeBase,
          url: requiredString(value, "url", path, type),
          ...(title === undefined ? {} : { title }),
          children: childNodes,
        };
      }
      const referenceType = referenceKind(value.referenceType, `${path}.referenceType`, type);
      const label = optionalString(value, "label", path, type);
      return {
        type,
        ...nodeBase,
        referenceType,
        identifier: requiredString(value, "identifier", path, type),
        ...(label === undefined ? {} : { label }),
        children: childNodes,
      };
    }
    case "containerDirective":
    case "leafDirective":
    case "textDirective": {
      assertOnlyKeys(
        value,
        ["type", "position", "data", "name", "attributes", "children"],
        path,
        type,
      );
      const shared = {
        type,
        ...nodeBase,
        name: requiredString(value, "name", path, type),
        attributes: directiveAttributes(value.attributes, `${path}.attributes`, type),
      };
      return type === "containerDirective"
        ? {
            ...shared,
            type,
            children: children(value.children, `${path}.children`, type, adaptFlow),
          }
        : {
            ...shared,
            type,
            children: children(value.children, `${path}.children`, type, adaptPhrasing),
          };
    }
    case "mdxJsxFlowElement":
    case "mdxJsxTextElement": {
      assertOnlyKeys(
        value,
        ["type", "position", "data", "name", "attributes", "children"],
        path,
        type,
      );
      const name = optionalString(value, "name", path, type) ?? null;
      const nodeAttributes = attributes(value.attributes, `${path}.attributes`, type);
      return type === "mdxJsxFlowElement"
        ? {
            type,
            ...nodeBase,
            name,
            attributes: nodeAttributes,
            children: children(value.children, `${path}.children`, type, adaptFlow),
          }
        : {
            type,
            ...nodeBase,
            name,
            attributes: nodeAttributes,
            children: children(value.children, `${path}.children`, type, adaptPhrasing),
          };
    }
    case "mdxFlowExpression":
    case "mdxTextExpression": {
      assertOnlyKeys(value, ["type", "position", "data", "value", "_markdownRsStops"], path, type);
      const expression = requiredString(value, "value", path, type);
      stops(value._markdownRsStops, `${path}._markdownRsStops`, type);
      return { type, ...nodeBase, value: expression };
    }
    case "definition": {
      assertOnlyKeys(
        value,
        ["type", "position", "data", "url", "title", "identifier", "label"],
        path,
        type,
      );
      const title = optionalString(value, "title", path, type);
      const label = optionalString(value, "label", path, type);
      return {
        type,
        ...nodeBase,
        url: requiredString(value, "url", path, type),
        ...(title === undefined ? {} : { title }),
        identifier: requiredString(value, "identifier", path, type),
        ...(label === undefined ? {} : { label }),
      };
    }
    case "image":
    case "imageReference": {
      const reference = type === "imageReference";
      assertOnlyKeys(
        value,
        reference
          ? ["type", "position", "data", "alt", "referenceType", "identifier", "label"]
          : ["type", "position", "data", "alt", "url", "title"],
        path,
        type,
      );
      const alt = requiredString(value, "alt", path, type);
      if (type === "image") {
        const title = optionalString(value, "title", path, type);
        return {
          type,
          ...nodeBase,
          alt,
          url: requiredString(value, "url", path, type),
          ...(title === undefined ? {} : { title }),
        };
      }
      const label = optionalString(value, "label", path, type);
      return {
        type,
        ...nodeBase,
        alt,
        referenceType: referenceKind(value.referenceType, `${path}.referenceType`, type),
        identifier: requiredString(value, "identifier", path, type),
        ...(label === undefined ? {} : { label }),
      };
    }
    case "footnoteReference": {
      assertOnlyKeys(value, ["type", "position", "data", "identifier", "label"], path, type);
      const label = optionalString(value, "label", path, type);
      return {
        type,
        ...nodeBase,
        identifier: requiredString(value, "identifier", path, type),
        ...(label === undefined ? {} : { label }),
      };
    }
    case "code": {
      assertOnlyKeys(value, ["type", "position", "data", "lang", "meta", "value"], path, type);
      const lang = optionalString(value, "lang", path, type);
      const meta = optionalString(value, "meta", path, type);
      return {
        type,
        ...nodeBase,
        ...(lang === undefined ? {} : { lang }),
        ...(meta === undefined ? {} : { meta }),
        value: requiredString(value, "value", path, type),
      };
    }
    case "html":
    case "text":
    case "inlineCode":
    case "yaml": {
      assertOnlyKeys(value, ["type", "position", "data", "value"], path, type);
      return { type, ...nodeBase, value: requiredString(value, "value", path, type) };
    }
    case "thematicBreak":
    case "break": {
      assertOnlyKeys(value, ["type", "position", "data"], path, type);
      return { type, ...nodeBase };
    }
    case "root":
      fail(path, type, "nested root nodes are not supported");
    case "math":
    case "inlineMath":
    case "toml":
    case "mdxjsEsm":
      fail(path, type, "node type is currently unsupported by the adapter");
    default:
      fail(path, type, "unknown node type");
  }
}

function referenceKind(value: unknown, path: string, nodeType: string) {
  if (value !== "shortcut" && value !== "collapsed" && value !== "full") {
    fail(path, nodeType, "expected shortcut, collapsed, or full");
  }
  return value;
}

/**
 * Validate a raw `parseToAst` tree and return a detached canonical mdast tree.
 *
 * This strict adapter is intentionally separate from the forward-compatible
 * raw tier: unsupported or malformed nodes throw instead of being dropped.
 */
export function toMdastRoot(ast: MdastRoot | null): EcosystemRoot {
  if (ast === null) fail("$", null, "expected a root node");
  const value: unknown = ast;
  const type = observedType(value);
  if (!isRecord(value) || type !== "root") fail("$", type, "expected a root node");
  assertOnlyKeys(value, ["type", "position", "data", "children"], "$", type);
  const nodeData = data(value, "$", type);
  return {
    type,
    position: position(value, "$", type),
    ...(nodeData === undefined ? {} : { data: nodeData }),
    children: children(value.children, "$.children", type, adaptTopLevel),
  };
}
