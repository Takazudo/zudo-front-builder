import { Island, type Child as RootChild, type IslandProps } from "@takazudo/zfb";
import {
  signal,
  type Child,
  type CSSProperties,
  type CssProperty,
  type ElementForTag,
  type HTMLAttributes,
  type InputProps,
  type Ref,
  type SelectProps,
  type Style,
  type SVGAttributes,
  type TextareaProps,
} from "@takazudo/zfb/zudo-react";
import type { JSX } from "@takazudo/zfb/zudo-react/jsx-runtime";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;

const anchorElement: Equal<ElementForTag<"a">, HTMLAnchorElement> = true;
const pathElement: Equal<ElementForTag<"path">, SVGPathElement> = true;
const rootChildAlias: Equal<RootChild, Child> = true;
const anchorRef: Equal<JSX.IntrinsicElements["a"]["ref"], Ref<HTMLAnchorElement> | undefined> =
  true;
const pathRef: Equal<JSX.IntrinsicElements["path"]["ref"], Ref<SVGPathElement> | undefined> = true;

function Link(props: HTMLAttributes<HTMLAnchorElement>) {
  return <a {...props} />;
}
function IconPath(props: SVGAttributes<SVGPathElement>) {
  return <path {...props} />;
}
const wrapped = (
  <>
    <Link
      href="/"
      ref={{ current: null }}
      on:click={(event) => {
        void event.currentTarget.href;
      }}
    />
    <IconPath
      d="M0 0"
      on:click={(event) => {
        void event.currentTarget.getTotalLength();
      }}
    />
  </>
);
const htmlAlias: JSX.HTMLAttributes<HTMLAnchorElement> = { href: "/" };
const svgAlias: JSX.SVGAttributes<SVGPathElement> = { d: "M0 0" };
const textModel = signal("text");
const checkedModel = signal(true);
const input: InputProps = { type: "text", modelValue: textModel };
const checkbox: InputProps = { type: "checkbox", modelChecked: checkedModel };
const textarea: TextareaProps = { modelValue: textModel };
const select: SelectProps = { modelValue: textModel };
const style: CSSProperties = { color: "red", "font-size": 12, "--theme": "dark" };
const styleAlias: JSX.CSSProperties = style;
const styleKey: CssProperty = "font-size";
const publicStyle: Style = style;

function Component() {
  return <div />;
}
const island = (
  <Island ssrFallback={<p>Loading</p>}>
    <Component />
  </Island>
);
// JSX expressions erase whether their tag was intrinsic; runtime rejects this child.
const erasedIntrinsicIsland = (
  <Island>
    <div />
  </Island>
);
const fallback: IslandProps = { children: <Component />, ssrFallback: ["loading", 0, false] };
const child: Child = fallback.ssrFallback;

// @ts-expect-error An arbitrary object is not an owned child.
const badFallback: IslandProps = { children: <Component />, ssrFallback: { text: "loading" } };
// @ts-expect-error Bigints are not owned children.
const badBigintFallback: IslandProps = { children: <Component />, ssrFallback: 1n };
// @ts-expect-error The island's component child cannot be a bigint.
const badBigintIsland = <Island>{1n}</Island>;
// @ts-expect-error A scalar cannot be the island's component child.
const badScalarIsland = <Island>{"text"}</Island>;
// @ts-expect-error A plain object cannot be the island's component child.
const badObjectIsland = <Island>{{ text: "value" }}</Island>;
const badAnchorRef: JSX.IntrinsicElements["a"] = {
  // @ts-expect-error A ref on an anchor must target HTMLAnchorElement.
  ref: { current: document.createElement("div") },
};
// @ts-expect-error The checked style surface rejects camelCase keys.
const badStyle: CSSProperties = { backgroundColor: "red" };
// @ts-expect-error The JSX alias has the same checked style surface.
const badAliasStyle: JSX.CSSProperties = { backgroundColor: "red" };
// @ts-expect-error Checkbox models are boolean signals.
const badCheckbox: InputProps = { type: "checkbox", modelChecked: textModel };
// @ts-expect-error Textarea models are string signals.
const badTextarea: TextareaProps = { modelValue: checkedModel };
// @ts-expect-error Unknown HTML attributes remain rejected.
const badHTML: HTMLAttributes<HTMLDivElement> = { className: "wrong dialect" };
// @ts-expect-error Unknown SVG attributes remain rejected.
const badSVG: SVGAttributes<SVGPathElement> = { strokeWidth: 2 };

void [
  anchorElement,
  pathElement,
  rootChildAlias,
  anchorRef,
  pathRef,
  wrapped,
  htmlAlias,
  svgAlias,
  input,
  checkbox,
  textarea,
  select,
  styleAlias,
  styleKey,
  publicStyle,
  island,
  erasedIntrinsicIsland,
  child,
  badFallback,
  badBigintFallback,
  badBigintIsland,
  badScalarIsland,
  badObjectIsland,
  badAnchorRef,
  badStyle,
  badAliasStyle,
  badCheckbox,
  badTextarea,
  badHTML,
  badSVG,
];
