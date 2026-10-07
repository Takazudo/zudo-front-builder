import { serializeStyle } from "./style.js";
import {
  Fragment,
  descriptionSite,
  isDescription,
  type Child,
  type Description,
  type DescriptionSite,
} from "./description.js";
import { escapeAttribute, escapeText } from "./escape.js";
import { rawHtmlReserved } from "./raw-html.js";
import type { IslandIdentity } from "./index.js";
import { readSnapshot } from "./reactive.js";
import type { ReadonlySignal } from "./reactive-types.js";
import { createScope, withScope, type RuntimeScope } from "./scope.js";
import { emptyChildren, normalizeProps } from "./props-transport.js";
import { islandRootType } from "./island-root-type.js";
import type { IslandOptions, RenderOptions } from "./server.js";
import {
  isShowType,
  isForType,
  showProps,
  forProps,
  keyed,
  keyPayload,
  view,
} from "./structure.js";
import {
  attributeError,
  attributeText,
  booleanAttrs,
  commonAttrs,
  dialectSuggestion,
  displayOnlyControl,
  emptyIframeChildren,
  formProps,
  htmlAttrs,
  htmlTags,
  isDialectProp,
  reactiveModelSuggestion,
  svgAttrs,
  svgTags,
  type Namespace,
  voidTags,
} from "./vocabulary.js";

const words = (value: string) => new Set(value.split(" "));
const reserved = words("key ref children rawHtml");
const sensitive = words("noscript xmp iframe noembed noframes plaintext");
const tableChildren: Record<string, Set<string>> = {
  table: words("caption colgroup thead tbody tfoot"),
  thead: words("tr"),
  tbody: words("tr"),
  tfoot: words("tr"),
  tr: words("td th"),
  colgroup: words("col"),
};
interface Context {
  identity: IslandIdentity | undefined;
  boundary: boolean;
  next: number;
  scope: RuntimeScope;
  path: string;
  node?: Description | undefined;
  selectValue?: string | undefined;
  selectSeen?: Set<string> | undefined;
  selectMatches?: number | undefined;
  radioGroups: Map<
    unknown,
    { name: string; form: string; value: string | null; options: Set<string> }
  >;
  radioNames: Map<string, unknown>;
  formId: string;
  nextFormId: number;
}
export interface RenderDiagnostic {
  readonly code: string;
  readonly path: string;
  readonly component: string;
  readonly spelling?: { readonly name: string; readonly suggestion?: string };
  readonly site?: DescriptionSite;
}
function fail(
  code: string,
  context: Context,
  detail: string,
  spelling?: RenderDiagnostic["spelling"],
): never {
  const component = context.identity?.component ?? "static render";
  const site = context.node && descriptionSite(context.node);
  const diagnostic: RenderDiagnostic = {
    code,
    path: context.path,
    component,
    ...(spelling && { spelling }),
    ...(site && { site }),
  };
  throw Object.assign(new TypeError(`${code}: ${detail} at ${context.path} in ${component}`), {
    diagnostic,
  });
}
function reactive(value: unknown): value is ReadonlySignal<unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    "$$zudoReactive" in value &&
    value.$$zudoReactive === "zudo-react.reactive.v1"
  );
}
function read(value: unknown): unknown {
  return reactive(value) ? readSnapshot(value) : value;
}
function region(context: Context, kind: string, content: () => string): string {
  if (!context.identity) return content();
  const id = context.next++;
  return `<!--zr:1:${id}:${kind}-->${content()}<!--/zr:1:${id}-->`;
}
function at<T>(context: Context, part: string, fn: () => T): T {
  const old = context.path;
  context.path += part;
  try {
    return fn();
  } finally {
    context.path = old;
  }
}
function children(value: unknown, context: Context, namespace: Namespace, parent: string): string {
  if (value == null || typeof value === "boolean") return "";
  if (Array.isArray(value))
    return value
      .map((item, index) =>
        at(context, `[${index}]`, () => render(item, context, namespace, parent)),
      )
      .join("");
  return render(value, context, namespace, parent);
}
function renderText(value: unknown, context: Context): string {
  if (value == null || typeof value === "boolean") return "";
  if (typeof value === "string" || (typeof value === "number" && Number.isFinite(value)))
    return escapeText(String(value));
  fail("ZR_CHILD", context, `invalid scalar ${typeof value}`);
}
function style(value: unknown, context: Context): string {
  try {
    return serializeStyle(value);
  } catch (error) {
    if (error instanceof TypeError)
      fail("ZR_STYLE", context, error.message.replace(/^ZR_STYLE: /, ""));
    throw error;
  }
}
function attributes(
  tag: string,
  props: Readonly<Record<string, unknown>>,
  context: Context,
  namespace: Namespace,
  custom: boolean,
): string {
  let output = "";
  for (const [name, original] of Object.entries(props)) {
    if (reserved.has(name)) continue;
    if (formProps.has(name)) continue;
    if (isDialectProp(name, namespace, custom)) {
      const suggestion = dialectSuggestion(name, namespace, custom);
      fail(
        "ZR_PROP_DIALECT",
        context,
        `${tag}.${name}${suggestion ? ` (use \`${suggestion}\` instead of \`${name}\`)` : ""}`,
        suggestion ? { name, suggestion } : { name },
      );
    }
    if (name.startsWith("on:")) {
      if (!/^on:[A-Za-z][A-Za-z0-9_-]*(?::capture)?$/.test(name))
        fail("ZR_LISTENER", context, `${tag}.${name}`);
      if (typeof original !== "function") fail("ZR_LISTENER", context, `${tag}.${name}`);
      const inline = `on${name.slice(3).replace(/:capture$/, "")}`;
      if (inline in props) fail("ZR_LISTENER_CONFLICT", context, `${tag}.${inline}`);
      continue;
    }
    if (!/^[A-Za-z_:][A-Za-z0-9_:.-]*$/.test(name)) fail("ZR_ATTRIBUTE", context, `${tag}.${name}`);
    if (
      !custom &&
      !commonAttrs.has(name) &&
      !(namespace === "svg" ? svgAttrs.has(name) : htmlAttrs.has(name)) &&
      !/^data-[\w.-]+$/.test(name) &&
      !/^aria-[\w.-]+$/.test(name) &&
      !/^on[a-z]+$/.test(name)
    )
      fail("ZR_ATTRIBUTE", context, `${tag}.${name}`);
    const displayValue = name === "value" && reactive(original) && displayOnlyControl(tag, props);
    if (displayValue && tag === "textarea") continue;
    const value = read(original);
    if ((name === "value" || name === "checked") && reactive(original) && !displayValue) {
      const suggestion = reactiveModelSuggestion(
        tag,
        name,
        String(read(props.type) ?? "text"),
        custom,
      );
      fail(
        "ZR_MODEL_UNSUPPORTED",
        context,
        `${tag}.${name} requires ${suggestion ?? "a static value"}`,
      );
    }
    if (displayValue && typeof value !== "string")
      fail("ZR_MODEL_VALUE", context, `${tag} requires string`);
    if (/^on[a-z]/.test(name) && typeof value === "function")
      fail("ZR_PROP_DIALECT", context, `${tag}.${name} must use on:${name.slice(2)}`, {
        name,
        suggestion: `on:${name.slice(2)}`,
      });
    const error = attributeError(name, value, custom);
    if (error) fail("ZR_ATTRIBUTE", context, `${tag}.${name} ${error}`);
    if (value == null) continue;
    const text = name === "style" ? style(value, context) : attributeText(name, value);
    if (text === null) continue;
    output +=
      booleanAttrs.has(name) && value === true ? ` ${name}` : ` ${name}="${escapeAttribute(text)}"`;
  }
  return output;
}
function restricted(
  value: unknown,
  context: Context,
  namespace: Namespace,
  parent: string,
): string {
  return tracked(value, context, () => restrictedValue(value, context, namespace, parent));
}
function restrictedValue(
  value: unknown,
  context: Context,
  namespace: Namespace,
  parent: string,
): string {
  if (value == null || typeof value === "boolean") return "";
  if (Array.isArray(value))
    return value
      .map((item, index) =>
        at(context, `[${index}]`, () => restricted(item, context, namespace, parent)),
      )
      .join("");
  if (typeof value === "string" || typeof value === "number") {
    if (parent in tableChildren && String(value).trim())
      fail("ZR_PARSER_CONTEXT", context, `${parent} text`);
    return renderText(value, context);
  }
  if (parent === "title" || parent === "option")
    fail("ZR_PARSER_CONTEXT", context, `${parent} requires static text`);
  if (!isDescription(value) || typeof value.type !== "string")
    fail("ZR_PARSER_CONTEXT", context, `${parent} requires intrinsic children`);
  return element(value, context, namespace, parent);
}
function element(
  description: Description,
  context: Context,
  namespace: Namespace,
  parent: string,
): string {
  const tag = description.type as string;
  const custom = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$/.test(tag);
  const elementNamespace = tag === "svg" || namespace === "svg" ? "svg" : "html";
  const childNamespace = tag === "foreignObject" ? "html" : elementNamespace;
  if (elementNamespace === "svg" ? !svgTags.has(tag) : !custom && !htmlTags.has(tag))
    fail("ZR_TAG", context, tag);
  if (context.identity && (tag === "template" || (tag !== "iframe" && sensitive.has(tag))))
    fail("ZR_PARSER_CONTEXT", context, tag);
  if (parent in tableChildren && !tableChildren[parent]?.has(tag))
    fail("ZR_PARSER_CONTEXT", context, `${parent} > ${tag}`);
  if (
    (parent === "select" && tag !== "option" && tag !== "optgroup") ||
    (parent === "optgroup" && tag !== "option")
  )
    fail("ZR_PARSER_CONTEXT", context, `${parent} > ${tag}`);
  const previousFormId = context.formId;
  if (tag === "form")
    context.formId =
      typeof description.props.id === "string"
        ? `id:${description.props.id}`
        : `form:${context.nextFormId++}`;
  const source = description.props;
  const props: Record<string, unknown> = { ...source };
  if (tag === "iframe") {
    if (Object.hasOwn(props, "rawHtml")) fail("ZR_RAW_HTML", context, "iframe forbids rawHtml");
    if (!emptyIframeChildren(props.children))
      fail("ZR_PARSER_CONTEXT", context, "iframe requires empty children");
  }
  const hasValue = source.modelValue !== undefined;
  const hasChecked = source.modelChecked !== undefined;
  if (
    (hasValue && hasChecked) ||
    (source.defaultValue !== undefined && source.value !== undefined) ||
    (source.defaultChecked !== undefined && source.checked !== undefined) ||
    (hasValue &&
      (source.defaultValue !== undefined ||
        (source.type !== "radio" && source.value !== undefined))) ||
    (hasChecked && (source.defaultChecked !== undefined || source.checked !== undefined))
  )
    fail("ZR_MODEL_CONFLICT", context, tag);
  if (hasValue || hasChecked) {
    for (const name of ["type", "multiple", "name", "form"])
      if (reactive(source[name])) fail("ZR_MODEL_UNSUPPORTED", context, `reactive ${name} shape`);
    const model = hasValue ? source.modelValue : source.modelChecked;
    let prototype = model;
    let writable = false;
    while (prototype && typeof prototype === "object") {
      const property = Object.getOwnPropertyDescriptor(prototype, "value");
      if (property) {
        writable = typeof property.set === "function";
        break;
      }
      prototype = Object.getPrototypeOf(prototype);
    }
    if (
      !reactive(model) ||
      !writable ||
      !("$$zudoWritable" in model) ||
      model.$$zudoWritable !== "zudo-react.writable.v1"
    )
      fail("ZR_MODEL_READONLY", context, tag);
  }
  if ((hasValue || hasChecked) && source.contenteditable !== undefined)
    fail("ZR_MODEL_UNSUPPORTED", context, `${tag}[contenteditable]`);
  const inputType = String(source.type ?? "text");
  if (
    hasValue &&
    !(
      tag === "textarea" ||
      tag === "select" ||
      (tag === "input" &&
        ["text", "search", "email", "url", "tel", "password", "radio"].includes(inputType))
    )
  )
    fail("ZR_MODEL_UNSUPPORTED", context, `${tag}[type=${inputType}]`);
  if (hasChecked && !(tag === "input" && inputType === "checkbox"))
    fail("ZR_MODEL_UNSUPPORTED", context, tag);
  if (
    source.defaultValue !== undefined &&
    !(tag === "textarea" || tag === "select" || tag === "input")
  )
    fail("ZR_MODEL_UNSUPPORTED", context, tag);
  if (
    source.defaultValue !== undefined &&
    tag === "input" &&
    !["text", "search", "email", "url", "tel", "password"].includes(inputType)
  )
    fail("ZR_MODEL_UNSUPPORTED", context, `${tag}[type=${inputType}]`);
  if (
    source.defaultChecked !== undefined &&
    !(tag === "input" && ["checkbox", "radio"].includes(inputType))
  )
    fail("ZR_MODEL_UNSUPPORTED", context, tag);
  if (tag === "input") {
    if (source.defaultValue !== undefined || (hasValue && inputType !== "radio"))
      props.value = read(source.modelValue ?? source.defaultValue);
    if (source.defaultChecked !== undefined || hasChecked)
      props.checked = read(source.modelChecked ?? source.defaultChecked);
    if (hasValue && inputType === "radio") {
      if (typeof source.name !== "string" || !source.name || typeof source.value !== "string")
        fail("ZR_MODEL_UNSUPPORTED", context, "radio requires static name and value");
      const selected = read(source.modelValue);
      if (selected !== null && typeof selected !== "string")
        fail("ZR_MODEL_VALUE", context, "radio requires string or null");
      const form = typeof source.form === "string" ? `id:${source.form}` : context.formId;
      const key = `${source.name}\u0000${form}`;
      const owner = context.radioNames.get(key);
      if (owner && owner !== source.modelValue)
        fail("ZR_MODEL_CONFLICT", context, "same-name radios use different models");
      context.radioNames.set(key, source.modelValue);
      const group = context.radioGroups.get(source.modelValue);
      if (
        group &&
        (group.name !== source.name || group.form !== form || group.options.has(source.value))
      )
        fail("ZR_MODEL_CONFLICT", context, "radio membership or duplicate value");
      if (group) group.options.add(source.value);
      else
        context.radioGroups.set(source.modelValue, {
          name: source.name,
          form,
          value: selected,
          options: new Set([source.value]),
        });
      props.checked = selected === source.value;
    }
  }
  if (tag === "option" && context.selectValue !== undefined) {
    const value = source.value;
    if (typeof value !== "string")
      fail("ZR_MODEL_UNSUPPORTED", context, "select option requires static value");
    if (context.selectSeen!.has(value))
      fail("ZR_MODEL_CONFLICT", context, "duplicate option value");
    context.selectSeen!.add(value);
    if (source.selected !== undefined)
      fail("ZR_MODEL_CONFLICT", context, "select-level value with option selected");
    props.selected = value === context.selectValue;
    if (props.selected) context.selectMatches = (context.selectMatches ?? 0) + 1;
  }
  if (context.boundary && ("data-zfb-island" in props || "data-zfb-island-skip-ssr" in props))
    fail("ZR_NESTED_ISLAND", context, tag);
  if (
    tag === "select" &&
    read(props.multiple) === true &&
    (context.identity || hasValue || source.defaultValue !== undefined)
  )
    fail("ZR_MODEL_UNSUPPORTED", context, "select[multiple]");
  const attrs = attributes(tag, props, context, elementNamespace, custom);
  const raw = props.rawHtml;
  const hasChildren = Object.hasOwn(props, "children");
  if (raw !== undefined && hasChildren)
    fail("ZR_RAW_HTML", context, `${tag} has children and rawHtml`);
  if (voidTags.has(tag) && (hasChildren || raw !== undefined))
    fail("ZR_VOID_CHILDREN", context, tag);
  if (
    raw !== undefined &&
    (elementNamespace === "svg" ||
      tag === "textarea" ||
      tag === "title" ||
      tag === "select" ||
      tag === "option")
  )
    fail("ZR_RAW_HTML", context, tag);
  const displayValue = reactive(source.value) && displayOnlyControl(tag, source);
  if (
    tag === "textarea" &&
    hasChildren &&
    (hasValue || source.defaultValue !== undefined || displayValue)
  )
    fail("ZR_MODEL_CONFLICT", context, tag);
  let content = "";
  if (raw !== undefined) {
    if ((tag === "script" || tag === "style") && reactive(raw))
      fail("ZR_RAW_HTML", context, `${tag} requires static rawHtml`);
    const payload = read(raw);
    if (typeof payload !== "string") fail("ZR_RAW_HTML", context, `${tag} requires string rawHtml`);
    if (rawHtmlReserved(payload, tag)) fail("ZR_RAW_HTML", context, "reserved boundary in rawHtml");
    if (
      (tag === "script" && /<\/script/i.test(payload)) ||
      (tag === "style" && /<\/style/i.test(payload))
    )
      fail("ZR_RAW_HTML", context, `closing ${tag} in rawHtml`);
    content = tag === "script" || tag === "style" ? payload : region(context, "h", () => payload);
  } else if (
    tag === "textarea" &&
    (hasValue || source.defaultValue !== undefined || displayValue)
  ) {
    const value = read(source.modelValue ?? source.defaultValue ?? source.value);
    if (typeof value !== "string") fail("ZR_MODEL_VALUE", context, "textarea requires string");
    content = `${value.startsWith("\n") ? "\n" : ""}${escapeText(value)}`;
  } else if (tag === "iframe") {
    // The shell has no owned children; srcdoc belongs to the browser's foreign document.
  } else if (tag === "script" || tag === "style") {
    if (hasChildren) fail("ZR_RAW_HTML", context, `${tag} requires rawHtml`);
  } else if (
    tag === "title" ||
    tag === "option" ||
    tag === "optgroup" ||
    tag === "select" ||
    tag in tableChildren
  ) {
    if (tag === "select" && (hasValue || source.defaultValue !== undefined)) {
      const value = read(source.modelValue ?? source.defaultValue);
      if (typeof value !== "string") fail("ZR_MODEL_VALUE", context, "select requires string");
      const oldValue = context.selectValue;
      const oldSeen = context.selectSeen;
      const oldMatches = context.selectMatches;
      context.selectValue = value;
      context.selectSeen = new Set();
      context.selectMatches = 0;
      try {
        content = restricted(props.children, context, childNamespace, tag);
        if (context.selectMatches !== 1)
          fail("ZR_MODEL_VALUE", context, `select has no option ${value}`);
      } finally {
        context.selectValue = oldValue;
        context.selectSeen = oldSeen;
        context.selectMatches = oldMatches;
      }
    } else content = restricted(props.children, context, childNamespace, tag);
  } else content = children(props.children, context, childNamespace, tag);
  if (tag === "pre" && raw === undefined && (content.startsWith("\n") || content.startsWith("\r")))
    content = `\n${content}`;
  context.formId = previousFormId;
  return `<${tag}${attrs}>${voidTags.has(tag) ? "" : `${content}</${tag}>`}`;
}
// A failure is attributed only to the value being rendered: a description's own site,
// or no site at all for scalars, promises and other non-description children.
function tracked<T>(value: unknown, context: Context, fn: () => T): T {
  const prior = context.node;
  context.node = isDescription(value) ? value : undefined;
  try {
    return fn();
  } finally {
    context.node = prior;
  }
}
function render(value: unknown, context: Context, namespace: Namespace, parent: string): string {
  return tracked(value, context, () => renderValue(value, context, namespace, parent));
}
function renderValue(
  value: unknown,
  context: Context,
  namespace: Namespace,
  parent: string,
): string {
  if (value == null || typeof value === "boolean") return "";
  if (Array.isArray(value))
    return region(context, "f", () => children(value, context, namespace, parent));
  if (reactive(value)) return region(context, "t", () => renderText(read(value), context));
  if (typeof value === "string" || typeof value === "number") return renderText(value, context);
  if (typeof value === "object" && value !== null && "then" in value)
    fail("ZR_ASYNC_COMPONENT", context, "promise child");
  if (!isDescription(value)) fail("ZR_CHILD", context, typeof value);
  if ((value.type as symbol | typeof value.type) === islandRootType)
    return island(value, context, namespace, parent);
  if (value.type === Fragment)
    return region(context, "f", () => children(value.props.children, context, namespace, parent));
  if (isShowType(value.type)) {
    if (
      parent in tableChildren ||
      ["select", "optgroup", "option", "title", "textarea", "script", "style"].includes(parent)
    )
      fail("ZR_PARSER_CONTEXT", context, parent);
    const props = showProps(value);
    if (
      !reactive(props.when) ||
      typeof props.children !== "function" ||
      (props.fallback !== undefined && typeof props.fallback !== "function")
    )
      fail("ZR_CHILD", context, "Show requires signal and factories");
    const selected = read(props.when);
    if (typeof selected !== "boolean") fail("ZR_CHILD", context, "Show.when requires boolean");
    return region(context, `s:${selected ? 1 : 0}`, () => {
      const factory = selected ? props.children : props.fallback;
      if (!factory) return "";
      const scope = context.scope.child("Show");
      const previous = context.scope;
      context.scope = scope;
      try {
        return withScope(scope, () => render(factory(), context, namespace, parent));
      } finally {
        context.scope = previous;
      }
    });
  }
  if (isForType(value.type)) {
    if (
      parent in tableChildren ||
      ["select", "optgroup", "option", "title", "textarea", "script", "style"].includes(parent)
    )
      fail("ZR_PARSER_CONTEXT", context, parent);
    const props = forProps<unknown>(value);
    if (
      !reactive(props.each) ||
      typeof props.by !== "function" ||
      typeof props.children !== "function"
    )
      fail("ZR_CHILD", context, "For requires signal, key and factory");
    const items = read(props.each);
    if (!Array.isArray(items)) fail("ZR_CHILD", context, "For.each requires array");
    const keys = keyed(items, props.by, context.path);
    return region(context, "l", () =>
      items
        .map((item, index) => {
          const scope = context.scope.child("For");
          const previous = context.scope;
          context.scope = scope;
          try {
            return region(context, `i:${keyPayload(keys[index]!)}`, () =>
              withScope(scope, () =>
                render(
                  props.children(view(item).value, view(index).value),
                  context,
                  namespace,
                  parent,
                ),
              ),
            );
          } finally {
            context.scope = previous;
          }
        })
        .join(""),
    );
  }
  if (typeof value.type === "string") return element(value, context, namespace, parent);
  const component = value.type;
  const scope = context.scope.child(component.name || "Anonymous");
  const prior = context.scope;
  context.scope = scope;
  try {
    return region(context, "c", () => {
      const output = withScope(scope, () => component(value.props));
      if (output && typeof output === "object" && "then" in output)
        fail("ZR_ASYNC_COMPONENT", context, component.name);
      return render(output, context, namespace, parent);
    });
  } finally {
    context.scope = prior;
  }
}
function island(
  description: Description,
  context: Context,
  namespace: Namespace,
  parent: string,
): string {
  if (context.boundary) fail("ZR_NESTED_ISLAND", context, "nested island");
  if (namespace !== "html" || parent === "p" || parent in tableChildren || parent === "select")
    fail("ZR_PARSER_CONTEXT", context, "island wrapper");
  const child = description.props.child;
  const options = description.props.options as IslandOptions;
  if (!isDescription(child) || typeof child.type !== "function")
    fail("ZR_ISLAND_CHILD", context, "one function component required");
  if (!options?.identity?.component || !options.identity.build)
    fail("ZR_ISLAND_IDENTITY", context, "component/build required");
  const component =
    (child.type as typeof child.type & { displayName?: string }).displayName ?? child.type.name;
  if (component !== options.identity.component)
    fail("ZR_ISLAND_IDENTITY", context, `expected ${options.identity.component}, got ${component}`);
  if (Object.hasOwn(child.props, "children") && !emptyChildren(child.props.children))
    fail("ZR_ISLAND_CHILD", context, "nonempty children");
  let props: Record<string, unknown>;
  let payload: string;
  try {
    props = normalizeProps(child.props);
    payload = JSON.stringify(props);
  } catch (error) {
    throw new TypeError(
      `ZR_ISLAND_PROPS ${component}: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  const when = options.when ?? "load";
  if (
    !["load", "idle", "visible", "media"].includes(when) ||
    (when === "media" && !options.media?.trim()) ||
    (when !== "media" && options.media !== undefined)
  )
    fail("ZR_ISLAND_WHEN", context, when);
  if (
    options.persist !== undefined &&
    (typeof options.persist !== "string" ||
      !options.persist.trim() ||
      options.persist !== options.persist.trim() ||
      /[\u0000-\u001f\u007f]/.test(options.persist))
  )
    fail(
      "ZR_ISLAND_PERSIST",
      context,
      "persist must be a nonempty string without surrounding whitespace or control characters",
    );
  if (options.persistProps !== undefined && typeof options.persistProps !== "boolean")
    fail("ZR_ISLAND_PERSIST", context, "persistProps must be a boolean");
  if (options.persistProps !== undefined && options.persist === undefined)
    fail("ZR_ISLAND_PERSIST", context, "persistProps requires persist");
  const marker = options.skipSsr ? "data-zfb-island-skip-ssr" : "data-zfb-island";
  const attrs = ` ${marker}="${escapeAttribute(options.identity.component)}" data-when="${when}"${when === "media" ? ` data-media="${escapeAttribute(options.media!)}"` : ""} data-zfb-transport="json/1" data-zfb-protocol="zudo-react/1" data-zfb-build="${escapeAttribute(options.identity.build)}" data-props="${escapeAttribute(payload)}"${options.persist === undefined ? "" : ` data-zfb-transition-persist="${escapeAttribute(options.persist)}"`}${options.persistProps === true ? ' data-zfb-transition-persist-props="true"' : ""}`;
  const local: Context = {
    radioGroups: new Map(),
    radioNames: new Map(),
    formId: "",
    nextFormId: 0,
    identity: options.skipSsr ? undefined : options.identity,
    boundary: true,
    next: 0,
    scope: context.scope,
    path: context.path,
  };
  const content = options.skipSsr
    ? children(options.fallback, local, namespace, "div")
    : render({ ...child, props }, local, "html", "div");
  validateRadioGroups(local);
  return `<div${attrs}>${content}</div>`;
}
function validateRadioGroups(context: Context): void {
  for (const group of context.radioGroups.values())
    if (group.value !== null && !group.options.has(group.value))
      fail("ZR_MODEL_VALUE", context, `radio has no option ${group.value}`);
}
export function renderHtml(node: Child, options: RenderOptions): string {
  const root = createScope({ component: options.island?.component ?? "<root>" });
  const context: Context = {
    radioGroups: new Map(),
    radioNames: new Map(),
    formId: "",
    nextFormId: 0,
    identity: options.island,
    boundary: options.island !== undefined,
    next: 0,
    scope: root,
    path: "root",
  };
  try {
    const html = render(node, context, "html", "");
    validateRadioGroups(context);
    return html;
  } finally {
    root.dispose();
  }
}
