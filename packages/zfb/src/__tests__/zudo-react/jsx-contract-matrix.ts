import { h, Fragment, type Child } from "../../zudo-react/index.js";
import { jsx, jsxs } from "../../zudo-react/jsx-runtime.js";
import { jsxDEV } from "../../zudo-react/jsx-dev-runtime.js";
import { defaultComponents } from "../../content.js";
import { Island } from "../../island.js";
import { ClientRouter } from "../../../../../packages/zfb-runtime/src/client-router-component.js";

export type JsxContractMatrixRow = {
  id: string;
  producerPath: string;
  symbol: string;
  emitted: string;
  contract: string;
  build: () => Child;
  reuse?: boolean;
  expectation:
    | { kind: "accepted"; html: string }
    | { kind: "rejected"; code: string; seam: "emitter" | "SDK" | "template" };
};

const accepted = (
  id: string,
  producerPath: string,
  symbol: string,
  emitted: string,
  contract: string,
  build: () => Child,
  html: string,
  extra: { reuse?: boolean } = {},
): JsxContractMatrixRow => ({
  id,
  producerPath,
  symbol,
  emitted,
  contract,
  build,
  expectation: { kind: "accepted", html },
  ...extra,
});

const rejected = (
  id: string,
  producerPath: string,
  symbol: string,
  emitted: string,
  contract: string,
  build: () => Child,
  code: string,
  seam: "emitter" | "SDK" | "template",
): JsxContractMatrixRow => ({
  id,
  producerPath,
  symbol,
  emitted,
  contract,
  build,
  expectation: { kind: "rejected", code, seam },
});

function Badge({ label }: { label: string }) {
  return h("strong", null, label);
}

function MemberContent({ children }: { children: Child }) {
  return h("article", null, children);
}

const moduleLevelIcon = h("svg", { viewBox: "0 0 1 1" }, h("path", { d: "M0 0" }));

const templateCharSetRows = [
  "crates/zfb/templates/basic-blog/layouts/default.tsx",
  "crates/zfb/templates/node-free/pages/index.tsx",
  "crates/zfb/templates/node-free/pages/posts/[slug].tsx",
].map((producerPath) =>
  rejected(
    `template charSet: ${producerPath}`,
    producerPath,
    "charSet",
    '<meta charSet="utf-8" />',
    "The dialect requires lowercase HTML `charset`.",
    () => h("meta", { charSet: "utf-8" }),
    "ZR_PROP_DIALECT",
    "template",
  ),
);

const templateDateTimeRows = [
  "crates/zfb/templates/basic-blog/pages/blog/[slug].tsx",
  "crates/zfb/templates/basic-blog/pages/index.tsx",
  "crates/zfb/templates/node-free/pages/index.tsx",
  "crates/zfb/templates/node-free/pages/posts/[slug].tsx",
].map((producerPath) =>
  rejected(
    `template dateTime: ${producerPath}`,
    producerPath,
    "dateTime",
    '<time dateTime="2026-09-28">today</time>',
    "The dialect requires lowercase HTML `datetime`.",
    () => h("time", { dateTime: "2026-09-28" }, "today"),
    "ZR_PROP_DIALECT",
    "template",
  ),
);

const templateKeyRows = [
  "crates/zfb/templates/basic-blog/pages/index.tsx",
  "crates/zfb/templates/basic-blog/pages/about.tsx",
  "crates/zfb/templates/basic-blog/pages/blog/[slug].tsx",
  "crates/zfb/templates/basic-blog/layouts/default.tsx",
  "crates/zfb/templates/node-free/pages/index.tsx",
].map((producerPath) =>
  accepted(
    `template keyed child: ${producerPath}`,
    producerPath,
    "key",
    '<li key="entry">item</li>',
    "The JSX factory's key argument is accepted and omitted from static HTML.",
    () => jsx("li", { children: "item" }, "entry"),
    "<li>item</li>",
  ),
);

const overrideChildren = (tag: string): Child => {
  if (tag === "ul" || tag === "ol") return h("li", null, "item");
  if (tag === "table") return h("tbody", null, h("tr", null, h("td", null, "item")));
  return tag;
};

const defaultComponentRows = Object.entries(defaultComponents).map(([tag, component]) => {
  const symbol = component.name;
  const child = overrideChildren(tag);
  const expectedChild =
    tag === "ul" || tag === "ol"
      ? "<li>item</li>"
      : tag === "table"
        ? "<tbody><tr><td>item</td></tr></tbody>"
        : tag;
  return accepted(
    `defaultComponents.${tag}`,
    "packages/zfb/src/content.ts",
    symbol,
    `defaultComponents.${tag} returns <${tag}> with forwarded props and children.`,
    "HTML-spelled passthrough props and valid child structure are accepted.",
    () => component({ id: "mapped", children: child as never }) as unknown as Child,
    `<${tag} id="mapped">${expectedChild}</${tag}>`,
  );
});

const clientRouterNodes = ClientRouter();

export const jsxContractMatrix: readonly JsxContractMatrixRow[] = [
  accepted(
    "synthetic tsconfig automatic JSX factory",
    "crates/zfb-build/src/bundler.rs",
    "write_synthetic_tsconfig",
    'compilerOptions.jsx = "react-jsx"; jsx("p", { children: "page" })',
    "The automatic runtime receives (type, props, key) and preserves HTML-spelled props.",
    () => jsx("p", { children: "page" }),
    "<p>page</p>",
  ),
  accepted(
    "esbuild automatic JSX and Fragment import",
    "crates/zfb-islands/src/esbuild.rs",
    "--jsx=automatic",
    'jsxs(Fragment, { children: [jsx("b", ...), jsx("i", ...)] })',
    "Fragment and static child arrays are accepted with no separator bytes.",
    () => jsxs(Fragment, { children: [jsx("b", { children: "a" }), jsx("i", { children: "b" })] }),
    "<b>a</b><i>b</i>",
  ),
  accepted(
    "MDX lowercase component-map entry",
    "crates/zfb-content/src/mdx_jsx_emit.rs",
    "emit_jsx",
    '_components.h1 resolves to the string "h1".',
    "A mapped lowercase tag remains a standard intrinsic element.",
    () => jsx("h1", { children: "Heading" }),
    "<h1>Heading</h1>",
  ),
  accepted(
    "MDX PascalCase component prop lookup",
    "crates/zfb-content/src/mdx_jsx_emit.rs",
    "emit_jsx",
    "Badge resolves from _components.Badge ?? components.Badge.",
    "A function component with scalar props is invoked once by the renderer.",
    () => jsx(Badge, { label: "parity" }),
    "<strong>parity</strong>",
  ),
  accepted(
    "MDX Fragment wrapper",
    "crates/zfb-content/src/mdx_jsx_emit.rs",
    "mdx_to_jsx_module",
    "The emitted _Fragment wraps the document body.",
    "A Fragment emits its children contiguously in static mode.",
    () => jsxs(Fragment, { children: [h("p", null, "one"), h("p", null, "two")] }),
    "<p>one</p><p>two</p>",
  ),
  rejected(
    "MDX inline raw HTML wrapper",
    "crates/zfb-content/src/mdx_jsx_emit.rs",
    "emit_node",
    'span dangerouslySetInnerHTML={{__html: "<em>raw</em>"}}',
    "The runtime reserves `rawHtml` for supported contexts; React's prop spelling is rejected.",
    () => h("span", { dangerouslySetInnerHTML: { __html: "<em>raw</em>" } }),
    "ZR_PROP_DIALECT",
    "emitter",
  ),
  rejected(
    "MDX block raw HTML wrapper",
    "crates/zfb-content/src/mdx_jsx_emit.rs",
    "emit_node",
    'div dangerouslySetInnerHTML={{__html: "<section>raw</section>"}}',
    "The runtime reserves `rawHtml` for supported contexts; React's prop spelling is rejected.",
    () => h("div", { dangerouslySetInnerHTML: { __html: "<section>raw</section>" } }),
    "ZR_PROP_DIALECT",
    "emitter",
  ),
  rejected(
    "MDX expression React class prop",
    "crates/zfb-content/src/mdx_jsx_emit.rs",
    "render_jsx_attrs",
    'className="custom"',
    "HTML `class` is accepted; React `className` is rejected.",
    () => h("p", { className: "custom" }, "text"),
    "ZR_PROP_DIALECT",
    "emitter",
  ),
  rejected(
    "MDX CSS object conversion",
    "crates/zfb-content/src/mdx_jsx_emit.rs",
    "jsx_style_attr",
    'style={{ backgroundColor: "red", "--shiki-dark-bg": "#111" }}',
    "The contract requires CSS-spelled `background-color`; camelCase object keys are rejected.",
    () => h("span", { style: { backgroundColor: "red", "--shiki-dark-bg": "#111" } }),
    "ZR_STYLE",
    "emitter",
  ),
  accepted(
    "MDX HAST boolean and empty data attributes",
    "crates/zfb-content/src/mdx_jsx_emit.rs",
    "render_hast_attrs",
    '<button disabled /> <span data-footnote-ref="" />',
    "Boolean HTML attributes use presence; an empty data attribute remains an empty string.",
    () =>
      jsxs(Fragment, {
        children: [h("button", { disabled: true }), h("span", { "data-footnote-ref": "" })],
      }),
    '<button disabled></button><span data-footnote-ref=""></span>',
  ),
  rejected(
    "generated Markdown shell with React charSet",
    "crates/zfb-build/src/bundler.rs",
    "render_md_page_shell",
    '<html><head><meta charSet="utf-8" /></head><body><template data-zfb-render-region="start" /></body></html>',
    "The frozen dialect requires lowercase `charset`.",
    () =>
      h(
        "html",
        { lang: "en" },
        h("head", null, h("meta", { charSet: "utf-8" })),
        h("body", null, h("template", { "data-zfb-render-region": "start" })),
      ),
    "ZR_PROP_DIALECT",
    "emitter",
  ),
  accepted(
    "generated Markdown shell sentinel child shape",
    "crates/zfb-build/src/bundler.rs",
    "render_md_page_shell",
    "The start template, body, and end template are JSX siblings.",
    "Static sentinel templates render as exact adjacent bytes.",
    () =>
      jsxs(Fragment, {
        children: [
          h("template", { "data-zfb-render-region": "start", "data-zfb-region-id": "mdx://p#1" }),
          h("p", null, "body"),
          h("template", { "data-zfb-render-region": "end", "data-zfb-region-id": "mdx://p#1" }),
        ],
      }),
    '<template data-zfb-render-region="start" data-zfb-region-id="mdx://p#1"></template><p>body</p><template data-zfb-render-region="end" data-zfb-region-id="mdx://p#1"></template>',
  ),
  accepted(
    "Content render-region Fragment and three children",
    "packages/zfb/src/content.ts",
    "wrapInRenderRegion",
    "jsxs(Fragment, { children: [start template, rendered child, end template] })",
    "Owned static rendering keeps both sentinel templates byte-adjacent to the region.",
    () =>
      jsxs(Fragment, {
        children: [
          h("template", { "data-zfb-render-region": "start", "data-zfb-region-id": "region-1" }),
          h("p", null, "region"),
          h("template", { "data-zfb-render-region": "end", "data-zfb-region-id": "region-1" }),
        ],
      }),
    '<template data-zfb-render-region="start" data-zfb-region-id="region-1"></template><p>region</p><template data-zfb-render-region="end" data-zfb-region-id="region-1"></template>',
  ),
  accepted(
    "Content mintElement template sentinel",
    "packages/zfb/src/content.ts",
    "mintElement",
    'jsx("template", { "data-zfb-render-region": edge, "data-zfb-region-id": id })',
    "A template element with scalar data attributes is accepted.",
    () => jsx("template", { "data-zfb-render-region": "start", "data-zfb-region-id": "region-1" }),
    '<template data-zfb-render-region="start" data-zfb-region-id="region-1"></template>',
  ),
  accepted(
    "Content fallback pre element",
    "packages/zfb/src/content.ts",
    "renderFallback",
    'jsx("pre", { "data-zfb-content-fallback": "", children: marker + "\\n" + body })',
    "The empty data attribute and preformatted fallback text are supported.",
    () => jsx("pre", { "data-zfb-content-fallback": "", children: "[zfb fallback render]\nbody" }),
    '<pre data-zfb-content-fallback="">[zfb fallback render]\nbody</pre>',
  ),
  ...defaultComponentRows,
  rejected(
    "default component React className passthrough",
    "packages/zfb/src/content.ts",
    "ContentParagraph",
    'ContentParagraph({ className: "custom" }) forwards className to <p>.',
    "The owned dialect requires the HTML spelling `class`.",
    () =>
      defaultComponents.p({
        className: "custom",
        children: "paragraph",
      } as never) as unknown as Child,
    "ZR_PROP_DIALECT",
    "SDK",
  ),
  accepted(
    "Island data attributes and JSON props",
    "packages/zfb/src/island.ts",
    "Island",
    'jsx("div", { "data-zfb-island": name, "data-when": when, "data-props": json, children })',
    "The wrapper's string data attributes and already JSON-encoded props are accepted.",
    () => {
      function Widget({ count }: { count: number }) {
        return h("b", null, count);
      }
      return Island({ children: jsx(Widget, { count: 3 }) as never }) as unknown as Child;
    },
    '<div data-zfb-island="Widget" data-when="load" data-props="{&quot;count&quot;:3}"><b>3</b></div>',
  ),
  accepted(
    "ClientRouter keyed sibling array",
    "packages/zfb-runtime/src/client-router-component.ts",
    "ClientRouter",
    "Returns a bare array of keyed meta elements plus a style element.",
    "A returned keyed array is accepted; static rendering omits keys.",
    () => clientRouterNodes.slice(1) as unknown as Child,
    '<meta name="zfb-view-transitions-enabled" content="true"><meta name="zfb-view-transitions-fallback" content="animate">',
  ),
  accepted(
    "ClientRouter style raw-content prop",
    "packages/zfb-runtime/src/client-router-component.ts",
    "makeVNode",
    'jsx("style", { rawHtml: announcerCss }, key)',
    "The owned factory selects trusted static `rawHtml` for the style element.",
    () => clientRouterNodes[0] as unknown as Child,
    `<style>${clientRouterNodes[0]?.props.rawHtml}</style>`,
  ),
  accepted(
    "template HTML class attribute",
    "crates/zfb/templates/basic-blog/pages/index.tsx",
    "class",
    '<h1 class="text-3xl">Welcome</h1>',
    "HTML `class` is accepted and preserved exactly.",
    () => h("h1", { class: "text-3xl" }, "Welcome"),
    '<h1 class="text-3xl">Welcome</h1>',
  ),
  ...templateCharSetRows,
  ...templateDateTimeRows,
  ...templateKeyRows,
  rejected(
    "template inline script raw HTML prop",
    "crates/zfb/templates/basic-blog/layouts/default.tsx",
    "dangerouslySetInnerHTML",
    "script dangerouslySetInnerHTML={{ __html: THEME_BOOTSTRAP_SCRIPT }}",
    "The React prop spelling is rejected; raw-text script content is separately restricted by ZR06.",
    () => h("script", { dangerouslySetInnerHTML: { __html: "window.theme='dark'" } }),
    "ZR_PROP_DIALECT",
    "template",
  ),
  accepted(
    "template script using the contract rawHtml prop",
    "crates/zfb/templates/basic-blog/layouts/default.tsx",
    "THEME_BOOTSTRAP_SCRIPT",
    "script rawHtml=\"window.theme='dark'\"",
    "ZR06 permits static string rawHtml in script raw-text context.",
    () => h("script", { rawHtml: "window.theme='dark'" }),
    "<script>window.theme='dark'</script>",
  ),
  ...[
    "crates/zfb/templates/basic-blog/components/theme-toggle.tsx",
    "crates/zfb/templates/basic-blog/components/callout.tsx",
  ].map((producerPath) =>
    accepted(
      `template kebab-case SVG presentation attribute: ${producerPath}`,
      producerPath,
      "stroke-width",
      '<path stroke-width="1.8" />',
      "The contract's SVG attribute table includes hyphenated presentation spellings.",
      () => h("svg", null, h("path", { "stroke-width": "1.8" })),
      '<svg><path stroke-width="1.8"></path></svg>',
    ),
  ),
  accepted(
    "module-level JSX reused across renders",
    "crates/zfb/templates/basic-blog/components/callout.tsx",
    "SPECS",
    "A module-level JSX path description is reused for each render.",
    "Descriptions are reusable; rendering does not cache request state on them.",
    () => moduleLevelIcon,
    '<svg viewBox="0 0 1 1"><path d="M0 0"></path></svg>',
    { reuse: true },
  ),
  accepted(
    "member-expression MDX component",
    "crates/zfb/templates/basic-blog/pages/blog/[slug].tsx",
    "post.Content",
    "<post.Content components={...}> resolves the member to a function component.",
    "Function components are valid element types when reached through a member expression.",
    () => jsx(MemberContent, { children: h("p", null, "MDX body") }),
    "<article><p>MDX body</p></article>",
  ),
  accepted(
    "null JSX child",
    "crates/zfb/templates/basic-blog/pages/index.tsx",
    "null",
    "The conditional child expression returns null when false.",
    "Null children are empty and emit no bytes.",
    () => h("p", null, null),
    "<p></p>",
  ),
  accepted(
    "plain TypeScript variadic h constructor",
    "crates/zfb/tests/page_extension_full_matrix_e2e.rs",
    "h",
    'h("div", null, "plain ", h("b", null, "page"))',
    "The variadic constructor preserves caller children in order.",
    () => h("div", null, "plain ", h("b", null, "page")),
    "<div>plain <b>page</b></div>",
  ),
  accepted(
    "development transform six-argument jsxDEV",
    "crates/zfb-md-wasm/tests/fixtures/parity/expected/mdx-components-expressions.json",
    "_jsxDEV",
    '_jsxDEV("pre", { class: "syntect-base16-ocean-dark", children: "code" }, void 0, false, source, this)',
    "The dev metadata arguments are accepted and do not alter runtime output.",
    () =>
      jsxDEV(
        "pre",
        { class: "syntect-base16-ocean-dark", children: "code" },
        undefined,
        false,
        { fileName: "components.mdx", lineNumber: 26, columnNumber: 7 },
        null,
      ),
    '<pre class="syntect-base16-ocean-dark">code</pre>',
  ),
];
