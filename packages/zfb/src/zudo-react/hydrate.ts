import { activateForms, prepareForms, reconcileForms, FormError, type FormPlan } from "./forms.js";
import { Fragment, isDescription, type Child, type Description } from "./description.js";
import { createScope, withScope, type RuntimeScope } from "./scope.js";
import { isReactive, read, styleText, setAttribute, bind, listener } from "./dom-bindings.js";
import {
  createRoot,
  diagnostic,
  report,
  rootPath,
  storedRoot,
  type RootHandle,
  type RootOptions,
} from "./root.js";
import type { ReadonlySignal } from "./reactive-types.js";

const HTML = "http://www.w3.org/1999/xhtml";
const SVG = "http://www.w3.org/2000/svg";
const voidTags = new Set(
  "area base br col embed hr img input link meta param source track wbr".split(" "),
);
const sensitive = new Set("template noscript xmp iframe noembed noframes plaintext".split(" "));
const tableChildren: Record<string, Set<string>> = {
  table: new Set("caption colgroup thead tbody tfoot".split(" ")),
  thead: new Set(["tr"]),
  tbody: new Set(["tr"]),
  tfoot: new Set(["tr"]),
  tr: new Set(["td", "th"]),
  colgroup: new Set(["col"]),
};
const restricted = new Set(
  "title select optgroup option table thead tbody tfoot tr colgroup".split(" "),
);
const booleanAttrs = new Set(
  "hidden inert readonly autofocus required disabled checked selected multiple open controls muted loop autoplay novalidate formnovalidate allowfullscreen".split(
    " ",
  ),
);
const dialect = new Set(
  "className htmlFor charSet dateTime tabIndex readOnly strokeWidth dangerouslySetInnerHTML".split(
    " ",
  ),
);
const formProps = new Set("modelValue modelChecked defaultValue defaultChecked".split(" "));
const shapeAttrs: Record<string, string[]> = {
  input: ["type", "form"],
  select: ["multiple", "form"],
  option: ["value"],
  textarea: ["form"],
};
const protocol = "zudo-react/1";
const words = (value: string) => new Set(value.split(" "));
const htmlTags = words(
  "html head body title base link meta style script div span p a br hr main header footer nav section article aside h1 h2 h3 h4 h5 h6 ul ol li dl dt dd blockquote pre code strong em b i small mark time figure figcaption img picture source video audio track canvas form label input button textarea select option optgroup fieldset legend output progress meter datalist table caption thead tbody tfoot tr th td col colgroup details summary dialog template slot iframe noscript address abbr bdi bdo cite data del dfn ins kbd map area object param q rp rt ruby s samp sub sup u var wbr embed xmp noembed noframes plaintext",
);
const svgTags = words(
  "svg g path circle ellipse rect line polyline polygon text tspan defs symbol use clipPath mask linearGradient radialGradient stop title desc foreignObject",
);
const commonAttrs = words(
  "id class title lang dir slot role style hidden inert contenteditable draggable spellcheck tabindex accesskey translate onclick onload onerror",
);
const htmlAttrs = words(
  "href target rel download src alt width height type name value placeholder for charset datetime readonly autofocus required disabled checked selected multiple open controls muted loop autoplay novalidate formnovalidate maxlength minlength min max step pattern autocomplete accept accept-charset http-equiv content media method action enctype rows cols colspan rowspan scope cite poster loading decoding sizes srcset crossorigin referrerpolicy sandbox allow allowfullscreen",
);
const svgAttrs = words(
  "viewBox preserveAspectRatio gradientUnits gradientTransform markerWidth markerHeight refX refY xlink:href xml:lang stroke-width fill-rule clip-rule stroke-linecap stroke-linejoin stop-color stop-opacity fill stroke d x y x1 x2 y1 y2 cx cy r rx ry points transform opacity offset",
);

type Operation = (map: Map<Node, Node>, cleanups: Array<() => void>) => void;
interface BuildContext {
  readonly document: Document;
  readonly options: RootOptions;
  readonly container: Element;
  readonly root: RuntimeScope;
  scope: RuntimeScope;
  next: number;
  readonly operations: Operation[];
  readonly forms: FormPlan[];
  readonly opaque: Set<Node>;
  readonly textSlots: Set<Node>;
}
class Failure extends Error {
  constructor(
    readonly code: string,
    readonly phase: "setup" | "preflight" | "commit" | "activation",
    readonly expected: string,
    readonly actual: string,
    readonly path: string,
  ) {
    super(`${code}: ${expected}; got ${actual}`);
  }
}
function fail(
  code: string,
  phase: Failure["phase"],
  expected: string,
  actual: string,
  path: string,
): never {
  throw new Failure(code, phase, expected, actual, path);
}
function append(parent: Node, node: Node): void {
  if (node.nodeType === Node.TEXT_NODE && parent.lastChild?.nodeType === Node.TEXT_NODE) {
    parent.lastChild.textContent = (parent.lastChild.textContent ?? "") + (node.textContent ?? "");
  } else parent.appendChild(node);
}
function marker(context: BuildContext, parent: Node, kind: string, content: () => void): void {
  const id = context.next++;
  const open = context.document.createComment(`zr:1:${id}:${kind}`);
  parent.appendChild(open);
  content();
  parent.appendChild(context.document.createComment(`/zr:1:${id}`));
  if (kind === "h") context.opaque.add(open);
  if (kind === "t") context.textSlots.add(open);
}
function scalar(value: unknown, path: string): string {
  if (value == null || typeof value === "boolean") return "";
  if (typeof value === "string" || (typeof value === "number" && Number.isFinite(value)))
    return String(value);
  fail("ZR_CHILD", "setup", "scalar", typeof value, path);
}
function children(
  value: unknown,
  parent: Node,
  context: BuildContext,
  namespace: string,
  tag: string,
  path: string,
): void {
  if (value == null || typeof value === "boolean") return;
  if (Array.isArray(value)) {
    value.forEach((entry, index) =>
      render(entry, parent, context, namespace, tag, `${path}[${index}]`),
    );
  } else render(value, parent, context, namespace, tag, path);
}
function validatePosition(tag: string, parent: string, context: BuildContext, path: string): void {
  if (sensitive.has(tag))
    fail("ZR_UNSUPPORTED_POSITION", "preflight", "hydratable element", tag, path);
  if (tableChildren[parent] && !tableChildren[parent].has(tag))
    fail("ZR_UNSUPPORTED_POSITION", "preflight", `valid ${parent} child`, tag, path);
  if (
    (parent === "select" && tag !== "option" && tag !== "optgroup") ||
    (parent === "optgroup" && tag !== "option")
  )
    fail("ZR_UNSUPPORTED_POSITION", "preflight", `valid ${parent} child`, tag, path);
}
function element(
  desc: Description,
  parent: Node,
  context: BuildContext,
  namespace: string,
  parentTag: string,
  path: string,
): void {
  const tag = desc.type as string;
  validatePosition(tag, parentTag, context, path);
  const ownNamespace = tag === "svg" || namespace === SVG ? SVG : HTML;
  const custom = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$/.test(tag);
  if (ownNamespace === SVG ? !svgTags.has(tag) : !custom && !htmlTags.has(tag))
    fail("ZR_TAG", "setup", "supported element", tag, path);
  const childNamespace = tag === "foreignObject" ? HTML : ownNamespace;
  const element = context.document.createElementNS(ownNamespace, tag);
  const props = desc.props;
  if (tag === "select" && read(props.multiple) === true)
    fail("ZR_MODEL_UNSUPPORTED", "preflight", "single select", "multiple", path);
  if (voidTags.has(tag) && ("children" in props || "rawHtml" in props))
    fail("ZR_VOID_CHILDREN", "setup", "empty element", tag, path);
  if ("rawHtml" in props && "children" in props)
    fail("ZR_RAW_HTML", "setup", "rawHtml without children", tag, path);
  if (
    tag === "textarea" &&
    "children" in props &&
    ("modelValue" in props || "defaultValue" in props)
  )
    fail(
      "ZR_MODEL_CONFLICT",
      "preflight",
      "textarea model/default without children",
      "children",
      path,
    );
  context.forms.push({ element, props, path });
  for (const [name, original] of Object.entries(props)) {
    if (name === "children" || name === "key" || name === "rawHtml") continue;
    if (formProps.has(name)) {
      if (name === "defaultValue" && tag === "input") setAttribute(element, "value", original);
      if (name === "defaultChecked" && tag === "input") setAttribute(element, "checked", original);
      if (name === "modelValue" && tag === "input" && read(props.type) !== "radio")
        setAttribute(element, "value", read(original));
      if (name === "modelChecked" && tag === "input")
        setAttribute(element, "checked", read(original));
      continue;
    }
    if (dialect.has(name) || /^on[A-Z]/.test(name))
      fail("ZR_PROP_DIALECT", "setup", "HTML-spelled prop", name, path);
    if (name === "ref") {
      if (!original || typeof original !== "object" || !("current" in original))
        fail("ZR_REF", "setup", "object ref", typeof original, path);
      const ref = original as { current: Element | null };
      context.operations.push((map, cleanups) => {
        const node = map.get(element) as Element;
        ref.current = node;
        cleanups.push(() => {
          if (ref.current === node) ref.current = null;
        });
      });
      continue;
    }
    if (name.startsWith("on:")) {
      const parsed = listener(name);
      if (!parsed || typeof original !== "function")
        fail("ZR_LISTENER", "setup", "native listener", name, path);
      context.operations.push((map, cleanups) => {
        const node = map.get(element) as Element;
        const handler = (event: Event) => {
          try {
            const result: unknown = (original as (event: Event) => unknown)(event);
            if (result && typeof result === "object" && "then" in result)
              Promise.resolve(result).catch((error) =>
                report(
                  context.options,
                  diagnostic(
                    context.options,
                    "ZR_EVENT_ERROR",
                    "update",
                    `${rootPath(context.container)}${path}`,
                    "handler success",
                    String(error),
                  ),
                ),
              );
          } catch (error) {
            report(
              context.options,
              diagnostic(
                context.options,
                "ZR_EVENT_ERROR",
                "update",
                `${rootPath(context.container)}${path}`,
                "handler success",
                String(error),
              ),
            );
          }
        };
        node.addEventListener(parsed.name, handler, parsed.capture);
        cleanups.push(() => node.removeEventListener(parsed.name, handler, parsed.capture));
      });
      continue;
    }
    if (name === "value" || name === "checked") {
      if (isReactive(original))
        fail("ZR_MODEL_UNSUPPORTED", "preflight", "modelValue/modelChecked", name, path);
    }
    if (
      !/^[A-Za-z_:][A-Za-z0-9_:.-]*$/.test(name) ||
      (!custom &&
        !commonAttrs.has(name) &&
        !(ownNamespace === SVG ? svgAttrs.has(name) : htmlAttrs.has(name)) &&
        !/^data-[\w.-]+$/.test(name) &&
        !/^aria-[\w.-]+$/.test(name) &&
        !/^on[a-z]+$/.test(name))
    )
      fail("ZR_ATTRIBUTE", "setup", "supported attribute", name, path);
    const initial = read(original);
    if (custom && initial != null && typeof initial !== "string")
      fail("ZR_ATTRIBUTE", "setup", "custom element string attribute", name, path);
    if (/^on[a-z]/.test(name) && typeof initial === "function")
      fail("ZR_PROP_DIALECT", "setup", "on:event", name, path);
    if (initial != null) {
      if (name === "style") styleText(initial);
      else if (booleanAttrs.has(name) && typeof initial !== "boolean")
        fail("ZR_ATTRIBUTE", "setup", "boolean", typeof initial, path);
      else if (
        typeof initial !== "string" &&
        typeof initial !== "number" &&
        typeof initial !== "boolean"
      )
        fail("ZR_ATTRIBUTE", "setup", "scalar", typeof initial, path);
      setAttribute(element, name, initial);
    }
    if (isReactive(original)) {
      const bindingScope = context.scope;
      context.operations.push((map, cleanups) => {
        const node = map.get(element) as Element;
        const subscription = bind(
          original,
          bindingScope,
          context.options,
          context.container,
          path,
          (value) => setAttribute(node, name, value),
          initial,
        );
        cleanups.push(() => subscription.dispose());
      });
    }
  }
  if ("rawHtml" in props) {
    const raw = props.rawHtml;
    if (ownNamespace === SVG || ["textarea", "title", "select", "option"].includes(tag))
      fail("ZR_RAW_HTML", "setup", "ordinary HTML container", tag, path);
    if ((tag === "script" || tag === "style") && isReactive(raw))
      fail("ZR_RAW_HTML", "setup", "static rawHtml", tag, path);
    const initial = read(raw);
    if (
      typeof initial !== "string" ||
      /<!--\/?zr:1:|data-zfb-island(?:-skip-ssr)?\s*=/.test(initial) ||
      (tag === "script" && /<\/script/i.test(initial)) ||
      (tag === "style" && /<\/style/i.test(initial))
    )
      fail("ZR_RAW_HTML", "setup", "valid rawHtml", typeof initial, path);
    if (tag === "script" || tag === "style") element.innerHTML = initial;
    else
      marker(context, element, "h", () => {
        element.insertAdjacentHTML("beforeend", initial);
      });
    const bindingScope = context.scope;
    if (isReactive(raw))
      context.operations.push((map, cleanups) => {
        const node = map.get(element) as Element;
        let previous = initial;
        const subscription = bind(
          raw as ReadonlySignal<string>,
          bindingScope,
          context.options,
          context.container,
          path,
          (value) => {
            if (value === previous) return;
            previous = value;
            const open = [...node.childNodes].find(
              (child) => child.nodeType === Node.COMMENT_NODE && child.textContent?.endsWith(":h"),
            );
            if (!open) return;
            let next = open.nextSibling;
            while (
              next &&
              !(next.nodeType === Node.COMMENT_NODE && next.textContent?.startsWith("/zr:1:"))
            ) {
              const current = next;
              next = next.nextSibling;
              current.parentNode?.removeChild(current);
            }
            const template = context.document.createElement("template");
            template.innerHTML = value;
            node.insertBefore(template.content, next);
          },
          initial,
        );
        cleanups.push(() => subscription.dispose());
      });
  } else if (
    tag === "textarea" &&
    (props.modelValue !== undefined || props.defaultValue !== undefined)
  ) {
    const value = String(read(props.modelValue ?? props.defaultValue));
    element.textContent = value;
  } else if (restricted.has(tag)) {
    if (tag === "title" || tag === "option") {
      const value = props.children;
      if (isReactive(value) || isDescription(value))
        fail("ZR_UNSUPPORTED_POSITION", "preflight", "static text", tag, path);
      append(element, context.document.createTextNode(scalar(value, path)));
    } else children(props.children, element, context, childNamespace, tag, `${path}/${tag}`);
  } else children(props.children, element, context, childNamespace, tag, `${path}/${tag}`);
  if (tag === "select" && (props.modelValue !== undefined || props.defaultValue !== undefined)) {
    const value = String(read(props.modelValue ?? props.defaultValue));
    const select = element as HTMLSelectElement;
    if (![...select.options].some((option) => option.value === value))
      fail("ZR_MODEL_VALUE", "preflight", "matching select option", value, path);
    for (const option of select.options) option.selected = option.value === value;
  }
  if (tag === "input" && read(props.type) === "radio" && props.modelValue !== undefined)
    setAttribute(element, "checked", read(props.modelValue) === read(props.value));
  parent.appendChild(element);
}
function render(
  value: unknown,
  parent: Node,
  context: BuildContext,
  namespace: string,
  parentTag: string,
  path: string,
): void {
  if (value == null || typeof value === "boolean") return;
  if (Array.isArray(value)) {
    marker(context, parent, "f", () =>
      children(value, parent, context, namespace, parentTag, path),
    );
    return;
  }
  if (isReactive(value)) {
    const initialValue = read(value) as string | number | boolean | null | undefined;
    const initial = scalar(initialValue, path);
    marker(context, parent, "t", () => {
      if (initial) parent.appendChild(context.document.createTextNode(initial));
    });
    const opener = parent.childNodes[parent.childNodes.length - (initial ? 3 : 2)]!;
    const bindingScope = context.scope;
    context.operations.push((map, cleanups) => {
      const start = map.get(opener)!;
      const end =
        start.nextSibling?.nodeType === Node.TEXT_NODE
          ? start.nextSibling.nextSibling
          : start.nextSibling;
      let textNode =
        start.nextSibling?.nodeType === Node.TEXT_NODE ? (start.nextSibling as Text) : null;
      if (!textNode) {
        textNode = context.document.createTextNode("");
        start.parentNode!.insertBefore(textNode, end ?? null);
      }
      const slot = textNode;
      const subscription = bind(
        value as ReadonlySignal<string | number | boolean | null | undefined>,
        bindingScope,
        context.options,
        context.container,
        path,
        (next) => {
          const text = scalar(next, path);
          if (slot.data !== text) slot.data = text;
        },
        initialValue,
      );
      cleanups.push(() => subscription.dispose());
    });
    return;
  }
  if (typeof value === "string" || typeof value === "number") {
    append(parent, context.document.createTextNode(scalar(value, path)));
    return;
  }
  if (!isDescription(value)) fail("ZR_CHILD", "setup", "description", typeof value, path);
  if (value.type === Fragment) {
    marker(context, parent, "f", () =>
      children(value.props.children, parent, context, namespace, parentTag, path),
    );
    return;
  }
  if (typeof value.type === "string") {
    element(value, parent, context, namespace, parentTag, path);
    return;
  }
  if (typeof value.type !== "function")
    fail("ZR_CHILD", "setup", "component", typeof value.type, path);
  if (restricted.has(parentTag))
    fail("ZR_UNSUPPORTED_POSITION", "preflight", "intrinsic child", "component", path);
  const component = value.type;
  const childScope = context.scope.child(component.name || "Anonymous");
  const previous = context.scope;
  context.scope = childScope;
  try {
    marker(context, parent, "c", () => {
      const output = withScope(childScope, () => component(value.props));
      if (output && typeof output === "object" && "then" in output)
        fail("ZR_ASYNC_COMPONENT", "setup", "synchronous component", "promise", path);
      render(output, parent, context, namespace, parentTag, path);
    });
  } finally {
    context.scope = previous;
  }
}
function normalize(text: string, pre: boolean): string {
  const line = text.replace(/\r\n?/g, "\n");
  return pre ? line : line.replace(/[\t\n\f\r ]+/g, " ").replace(/^ | $/g, "");
}
function compare(
  expected: Node,
  actual: Node,
  map: Map<Node, Node>,
  context: BuildContext,
  path: string,
  pre = false,
): void {
  if (expected.nodeType !== actual.nodeType)
    fail("ZR_HYDRATION_MISMATCH", "preflight", expected.nodeName, actual.nodeName, path);
  map.set(expected, actual);
  if (expected.nodeType === Node.TEXT_NODE) {
    if (normalize(expected.textContent ?? "", pre) !== normalize(actual.textContent ?? "", pre))
      fail("ZR_HYDRATION_MISMATCH", "preflight", "matching text", "different text", path);
    return;
  }
  if (expected.nodeType === Node.COMMENT_NODE) {
    if (expected.textContent !== actual.textContent)
      fail(
        "ZR_HYDRATION_MISMATCH",
        "preflight",
        expected.textContent ?? "",
        actual.textContent ?? "",
        path,
      );
    return;
  }
  if (expected.nodeType === Node.ELEMENT_NODE) {
    const wanted = expected as Element;
    const found = actual as Element;
    if (wanted.localName !== found.localName || wanted.namespaceURI !== found.namespaceURI)
      fail(
        "ZR_HYDRATION_MISMATCH",
        "preflight",
        `${wanted.namespaceURI}:${wanted.localName}`,
        `${found.namespaceURI}:${found.localName}`,
        path,
      );
    const shape = [...(shapeAttrs[wanted.localName] ?? [])];
    if (wanted.localName === "input" && (wanted.getAttribute("type") ?? "text") === "radio")
      shape.push("name", "value");
    for (const name of shape) {
      const expectedValue =
        name === "type" && wanted.localName === "input"
          ? (wanted.getAttribute(name) ?? "text")
          : wanted.getAttribute(name);
      const actualValue =
        name === "type" && wanted.localName === "input"
          ? (found.getAttribute(name) ?? "text")
          : found.getAttribute(name);
      if (expectedValue !== actualValue)
        fail(
          "ZR_HYDRATION_MISMATCH",
          "preflight",
          `${name}=${expectedValue}`,
          `${name}=${actualValue}`,
          path,
        );
    }
    if (wanted.localName === "script" || wanted.localName === "style") return;
    compareChildren(wanted, found, map, context, path, pre || wanted.localName === "pre");
  }
}
function significant(nodes: Node[], pre: boolean): Node[] {
  return pre
    ? nodes
    : nodes.filter(
        (node) =>
          node.nodeType !== Node.TEXT_NODE || normalize(node.textContent ?? "", false) !== "",
      );
}
function matchingClose(nodes: Node[], start: number): number {
  const open = nodes[start]!.textContent ?? "";
  const id = /^zr:1:(\d+):/.exec(open)?.[1];
  if (id === undefined) return -1;
  let depth = 0;
  for (let i = start + 1; i < nodes.length; i++) {
    const value = nodes[i]!.textContent;
    if (value?.startsWith("zr:1:")) depth++;
    else if (value?.startsWith("/zr:1:")) {
      if (depth) depth--;
      else return value === `/zr:1:${id}` ? i : -1;
    }
  }
  return -1;
}
function compareChildren(
  expected: Node,
  actual: Node,
  map: Map<Node, Node>,
  context: BuildContext,
  path: string,
  pre = false,
): void {
  const wants = significant([...expected.childNodes], pre);
  const has = significant([...actual.childNodes], pre);
  let i = 0;
  let j = 0;
  while (i < wants.length && j < has.length) {
    const wanted = wants[i]!;
    const found = has[j]!;
    const childPath = `${path}/${wanted.nodeName.toLowerCase()}[${i}]`;
    if (context.opaque.has(wanted)) {
      compare(wanted, found, map, context, childPath, pre);
      const wantedEnd = matchingClose(wants, i);
      const foundEnd = matchingClose(has, j);
      if (wantedEnd < 0 || foundEnd < 0)
        fail(
          "ZR_HYDRATION_MISMATCH",
          "preflight",
          "balanced rawHtml marker",
          "missing marker",
          childPath,
        );
      map.set(wants[wantedEnd]!, has[foundEnd]!);
      i = wantedEnd + 1;
      j = foundEnd + 1;
      continue;
    }
    if (context.textSlots.has(wanted)) {
      compare(wanted, found, map, context, childPath, pre);
      const wantedEnd = matchingClose(wants, i);
      const foundEnd = matchingClose(has, j);
      if (wantedEnd !== i + 1 && wantedEnd !== i + 2)
        fail("ZR_HYDRATION_MISMATCH", "preflight", "single text slot", "invalid slot", childPath);
      if (foundEnd !== j + 1 && foundEnd !== j + 2)
        fail("ZR_HYDRATION_MISMATCH", "preflight", "single text slot", "invalid slot", childPath);
      if (wantedEnd === i + 2) {
        if (foundEnd !== j + 2)
          fail("ZR_HYDRATION_MISMATCH", "preflight", "text slot", "empty slot", childPath);
        compare(wants[i + 1]!, has[j + 1]!, map, context, childPath, pre);
      } else if (foundEnd === j + 2 && normalize(has[j + 1]!.textContent ?? "", pre) !== "")
        fail("ZR_HYDRATION_MISMATCH", "preflight", "empty slot", "nonempty slot", childPath);
      map.set(wants[wantedEnd]!, has[foundEnd]!);
      i = wantedEnd + 1;
      j = foundEnd + 1;
      continue;
    }
    compare(wanted, found, map, context, childPath, pre);
    i++;
    j++;
  }
  if (i !== wants.length || j !== has.length)
    fail(
      "ZR_HYDRATION_MISMATCH",
      "preflight",
      `${wants.length} children`,
      `${has.length} children`,
      path,
    );
}
function validate(container: Element, options: RootOptions, mode: "hydrate" | "mount"): void {
  const path = rootPath(container);
  if (storedRoot(container) && !storedRoot(container)!.disposed)
    fail("ZR_DUPLICATE_ROOT", "preflight", "unowned root", "live root", path);
  if (options.signal?.aborted || !container.isConnected)
    fail("ZR_CANCELLED", "preflight", "connected root", "aborted or disconnected", path);
  const component =
    container.getAttribute(mode === "hydrate" ? "data-zfb-island" : "data-zfb-island-skip-ssr") ??
    container.getAttribute("data-zfb-island");
  const transport = container.getAttribute("data-zfb-transport");
  const serverProtocol = container.getAttribute("data-zfb-protocol");
  const build = container.getAttribute("data-zfb-build");
  if (
    component !== options.identity.component ||
    transport !== "json/1" ||
    serverProtocol !== protocol ||
    build !== options.identity.build
  )
    fail(
      "ZR_IDENTITY",
      "preflight",
      `${options.identity.component}/${protocol}/${options.identity.build}/json/1`,
      `${component}/${serverProtocol}/${build}/${transport}`,
      path,
    );
  if (container.querySelector("[data-zfb-island], [data-zfb-island-skip-ssr]"))
    fail("ZR_UNSUPPORTED_POSITION", "preflight", "no nested root", "nested root", path);
}
function execute(
  node: Child,
  container: Element,
  options: RootOptions,
  mode: "hydrate" | "mount",
): RootHandle | null {
  let scope: RuntimeScope | undefined;
  const cleanups: Array<() => void> = [];
  let committed = false;
  try {
    validate(container, options, mode);
    if (
      !isDescription(node) ||
      typeof node.type !== "function" ||
      ((node.type as typeof node.type & { displayName?: string }).displayName ?? node.type.name) !==
        options.identity.component
    )
      fail(
        "ZR_IDENTITY",
        "preflight",
        options.identity.component,
        "different child component",
        rootPath(container),
      );
    scope = createScope({
      component: options.identity.component,
      ...(options.report ? { reporter: options.report } : {}),
      protocol,
      build: options.identity.build,
      path: rootPath(container),
    });
    const context: BuildContext = {
      document: container.ownerDocument,
      options,
      container,
      root: scope,
      scope,
      next: 0,
      operations: [],
      forms: [],
      opaque: new Set(),
      textSlots: new Set(),
    };
    const fragment = container.ownerDocument.createDocumentFragment();
    render(node, fragment, context, HTML, "", rootPath(container));
    const built = [...fragment.childNodes];
    const map = new Map<Node, Node>();
    if (mode === "hydrate") compareChildren(fragment, container, map, context, rootPath(container));
    else {
      for (const expected of built) {
        const visit = (item: Node): void => {
          map.set(item, item);
          for (const child of item.childNodes) visit(child);
        };
        visit(expected);
      }
    }
    const formBindings = prepareForms(context.forms, map, mode, container);
    if (options.signal?.aborted || !container.isConnected)
      fail(
        "ZR_CANCELLED",
        "commit",
        "connected root",
        "aborted or disconnected",
        rootPath(container),
      );
    reconcileForms(formBindings, mode);
    if (mode === "mount") container.replaceChildren(fragment);
    committed = true;
    for (const operation of context.operations) operation(map, cleanups);
    activateForms(formBindings, scope, cleanups, options, container);
    const owned = mode === "hydrate" ? [...container.childNodes] : built;
    const handle = createRoot(container, options, scope, cleanups, owned);
    try {
      scope.activate();
    } catch (error) {
      handle.dispose();
      report(
        options,
        diagnostic(
          options,
          "ZR_ACTIVATION_ERROR",
          "activation",
          rootPath(container),
          "activation",
          String(error),
        ),
      );
      return null;
    }
    return handle;
  } catch (error) {
    for (const cleanup of [...cleanups].reverse())
      try {
        cleanup();
      } catch {
        /* preserve original error */
      }
    scope?.dispose();
    const failure =
      error instanceof Failure
        ? error
        : error instanceof FormError
          ? new Failure(error.code, "preflight", "valid form model", error.detail, error.path)
          : new Failure(
              "ZR_HYDRATION_MISMATCH",
              committed ? "commit" : "setup",
              "successful root",
              String(error),
              rootPath(container),
            );
    report(
      options,
      diagnostic(
        options,
        failure.code,
        failure.phase,
        failure.path,
        failure.expected,
        failure.actual,
      ),
    );
    return null;
  }
}
export function hydrate(node: Child, container: Element, options: RootOptions): RootHandle | null {
  return execute(node, container, options, "hydrate");
}
export function mount(node: Child, container: Element, options: RootOptions): RootHandle | null {
  return execute(node, container, options, "mount");
}
