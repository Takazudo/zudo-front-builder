import type { Child, Component, Description, Key } from "./description.js";
import type { Listener, Ref } from "./index.js";
import type { ReadonlySignal, Signal } from "./reactive-types.js";

type Value<T> = T | ReadonlySignal<T> | undefined;
type ScalarAttribute = Value<string | number | boolean | null>;
type DimensionAttribute = Value<string | number | null>;
type StringAttribute = Value<string | null>;
type BooleanAttribute = Value<boolean | null>;
export type CssProperty =
  | `--${string}`
  | `${string}-${string}`
  | "color"
  | "display"
  | "opacity"
  | "width"
  | "height"
  | "margin"
  | "padding"
  | "border"
  | "background"
  | "position"
  | "top"
  | "right"
  | "bottom"
  | "left"
  | "overflow"
  | "transform"
  | "transition"
  | "animation"
  | "font"
  | "flex"
  | "grid"
  | "gap";
export type CSSProperties = Readonly<
  Partial<Record<CssProperty, string | number | null | undefined>>
>;
// The custom-name index must also accept every mapped listener under strict function variance.
type AnyListener = Listener<any>;
type EventProps<TEl extends Element, TMap extends Record<keyof TMap, Event>> = {
  [K in keyof TMap & string as `on:${K}` | `on:${K}:capture`]?: Listener<
    TMap[K] & { currentTarget: TEl }
  >;
} & {
  [name: `on:${string}`]: AnyListener | undefined;
};
type DataAria = {
  [name: `data-${string}`]: ScalarAttribute;
} & {
  [name: `aria-${string}`]: ScalarAttribute;
};

interface ReservedProps<T extends Element = Element> {
  children?: Child | undefined;
  key?: Key | undefined;
  ref?: Ref<T> | undefined;
  rawHtml?: Value<string | null>;
}

interface CommonAttributes<T extends Element = Element> extends ReservedProps<T> {
  id?: StringAttribute;
  class?: StringAttribute;
  title?: StringAttribute;
  lang?: StringAttribute;
  dir?: StringAttribute;
  slot?: StringAttribute;
  role?: StringAttribute;
  style?: Value<string | CSSProperties | null>;
  hidden?: BooleanAttribute;
  inert?: BooleanAttribute;
  contenteditable?: Value<string | boolean | null>;
  draggable?: Value<string | boolean | null>;
  spellcheck?: Value<string | boolean | null>;
  tabindex?: Value<number | null>;
  accesskey?: StringAttribute;
  translate?: StringAttribute;
  onclick?: string | undefined;
  onload?: string | undefined;
  onerror?: string | undefined;
}

interface HtmlBaseAttributes<T extends HTMLElement = HTMLElement> extends CommonAttributes<T> {
  href?: StringAttribute;
  target?: StringAttribute;
  rel?: StringAttribute;
  download?: Value<string | boolean | null>;
  property?: StringAttribute;
  itemprop?: StringAttribute;
  hreflang?: StringAttribute;
  as?: StringAttribute;
  integrity?: StringAttribute;
  async?: BooleanAttribute;
  defer?: BooleanAttribute;
  nonce?: StringAttribute;
  popover?: StringAttribute;
  popovertarget?: StringAttribute;
  popovertargetaction?: StringAttribute;
  srcdoc?: StringAttribute;
  fetchpriority?: StringAttribute;
  inputmode?: StringAttribute;
  autocapitalize?: StringAttribute;
  form?: StringAttribute;
  label?: StringAttribute;
  preload?: StringAttribute;
  playsinline?: BooleanAttribute;
  wrap?: StringAttribute;
  closedby?: StringAttribute;
  src?: StringAttribute;
  alt?: StringAttribute;
  width?: Value<string | number | null>;
  height?: Value<string | number | null>;
  type?: StringAttribute;
  name?: StringAttribute;
  value?: string | number | null | undefined;
  placeholder?: StringAttribute;
  for?: StringAttribute;
  charset?: StringAttribute;
  datetime?: StringAttribute;
  readonly?: BooleanAttribute;
  autofocus?: BooleanAttribute;
  required?: BooleanAttribute;
  disabled?: BooleanAttribute;
  checked?: boolean | null | undefined;
  selected?: BooleanAttribute;
  multiple?: BooleanAttribute;
  reversed?: BooleanAttribute;
  open?: BooleanAttribute;
  controls?: BooleanAttribute;
  muted?: BooleanAttribute;
  loop?: BooleanAttribute;
  autoplay?: BooleanAttribute;
  novalidate?: BooleanAttribute;
  formnovalidate?: BooleanAttribute;
  maxlength?: Value<number | null>;
  minlength?: Value<number | null>;
  min?: StringAttribute;
  max?: StringAttribute;
  step?: StringAttribute;
  start?: Value<string | number | null>;
  pattern?: StringAttribute;
  autocomplete?: StringAttribute;
  accept?: StringAttribute;
  "accept-charset"?: StringAttribute;
  "http-equiv"?: StringAttribute;
  content?: StringAttribute;
  media?: StringAttribute;
  method?: StringAttribute;
  action?: StringAttribute;
  enctype?: StringAttribute;
  rows?: Value<number | null>;
  cols?: Value<number | null>;
  colspan?: Value<number | null>;
  rowspan?: Value<number | null>;
  scope?: StringAttribute;
  cite?: StringAttribute;
  poster?: StringAttribute;
  loading?: StringAttribute;
  decoding?: StringAttribute;
  sizes?: StringAttribute;
  srcset?: StringAttribute;
  crossorigin?: StringAttribute;
  referrerpolicy?: StringAttribute;
  sandbox?: StringAttribute;
  allow?: StringAttribute;
  allowfullscreen?: BooleanAttribute;
}

export type HTMLAttributes<T extends HTMLElement = HTMLElement> = HtmlBaseAttributes<T> &
  DataAria &
  EventProps<T, HTMLElementEventMap>;
type RawTextProps<T extends HTMLElement = HTMLElement> = Omit<
  HtmlBaseAttributes<T>,
  "children" | "rawHtml"
> &
  DataAria &
  EventProps<T, HTMLElementEventMap> & {
    children?: never;
    rawHtml?: string | undefined;
  };
type IframeProps = Omit<HtmlBaseAttributes<HTMLIFrameElement>, "children" | "rawHtml"> &
  DataAria &
  EventProps<HTMLIFrameElement, HTMLElementEventMap> & {
    children?: never;
    rawHtml?: never;
  };
type InputBase = Omit<HtmlBaseAttributes<HTMLInputElement>, "type"> &
  DataAria &
  EventProps<HTMLInputElement, HTMLElementEventMap> & {
    defaultValue?: string | undefined;
    defaultChecked?: boolean | undefined;
  };
export type InputProps = InputBase &
  (
    | {
        type?: "text" | "search" | "email" | "url" | "tel" | "password" | undefined;
        modelValue?: Signal<string> | undefined;
        modelChecked?: never;
      }
    | {
        type: "checkbox";
        modelChecked?: Signal<boolean> | undefined;
        modelValue?: never;
      }
    | {
        type: "radio";
        name: string;
        value: string;
        modelValue?: Signal<string | null> | undefined;
        modelChecked?: never;
      }
    | {
        type: string;
        modelValue?: never;
        modelChecked?: never;
      }
  );
export type TextareaProps = HTMLAttributes<HTMLTextAreaElement> & {
  modelValue?: Signal<string> | undefined;
  defaultValue?: string | undefined;
};
export type SelectProps = HTMLAttributes<HTMLSelectElement> & {
  modelValue?: Signal<string> | undefined;
  defaultValue?: string | undefined;
};

interface SvgBaseAttributes<T extends SVGElement = SVGElement> extends CommonAttributes<T> {
  width?: DimensionAttribute;
  height?: DimensionAttribute;
  viewBox?: StringAttribute;
  preserveAspectRatio?: StringAttribute;
  gradientUnits?: StringAttribute;
  gradientTransform?: StringAttribute;
  markerWidth?: ScalarAttribute;
  markerHeight?: ScalarAttribute;
  refX?: ScalarAttribute;
  refY?: ScalarAttribute;
  "xlink:href"?: StringAttribute;
  "xml:lang"?: StringAttribute;
  xmlns?: StringAttribute;
  "xmlns:xlink"?: StringAttribute;
  href?: StringAttribute;
  focusable?: StringAttribute;
  "stroke-width"?: ScalarAttribute;
  "fill-rule"?: StringAttribute;
  "clip-rule"?: StringAttribute;
  "stroke-linecap"?: StringAttribute;
  "stroke-linejoin"?: StringAttribute;
  "stop-color"?: StringAttribute;
  "stop-opacity"?: ScalarAttribute;
  "fill-opacity"?: ScalarAttribute;
  "stroke-dasharray"?: StringAttribute;
  "text-anchor"?: StringAttribute;
  fill?: StringAttribute;
  stroke?: StringAttribute;
  d?: StringAttribute;
  x?: ScalarAttribute;
  y?: ScalarAttribute;
  x1?: ScalarAttribute;
  x2?: ScalarAttribute;
  y1?: ScalarAttribute;
  y2?: ScalarAttribute;
  cx?: ScalarAttribute;
  cy?: ScalarAttribute;
  r?: ScalarAttribute;
  rx?: ScalarAttribute;
  ry?: ScalarAttribute;
  points?: StringAttribute;
  transform?: StringAttribute;
  opacity?: ScalarAttribute;
  offset?: ScalarAttribute;
  id?: StringAttribute;
}

export type SVGAttributes<T extends SVGElement = SVGElement> = SvgBaseAttributes<T> &
  DataAria &
  EventProps<T, SVGElementEventMap>;
type CustomProps = ReservedProps &
  EventProps<HTMLElement, GlobalEventHandlersEventMap> & {
    value?: string | null | undefined;
    checked?: null | undefined;
    [attribute: string]: StringAttribute | Child | Ref<Element> | AnyListener | undefined;
  };

type HtmlTag =
  | "html"
  | "head"
  | "body"
  | "title"
  | "base"
  | "link"
  | "meta"
  | "style"
  | "script"
  | "div"
  | "span"
  | "p"
  | "a"
  | "br"
  | "hr"
  | "main"
  | "header"
  | "footer"
  | "nav"
  | "section"
  | "article"
  | "aside"
  | "search"
  | "hgroup"
  | "menu"
  | "h1"
  | "h2"
  | "h3"
  | "h4"
  | "h5"
  | "h6"
  | "ul"
  | "ol"
  | "li"
  | "dl"
  | "dt"
  | "dd"
  | "blockquote"
  | "pre"
  | "code"
  | "strong"
  | "em"
  | "b"
  | "i"
  | "small"
  | "mark"
  | "time"
  | "figure"
  | "figcaption"
  | "img"
  | "picture"
  | "source"
  | "video"
  | "audio"
  | "track"
  | "canvas"
  | "form"
  | "label"
  | "input"
  | "button"
  | "textarea"
  | "select"
  | "option"
  | "optgroup"
  | "fieldset"
  | "legend"
  | "output"
  | "progress"
  | "meter"
  | "datalist"
  | "table"
  | "caption"
  | "thead"
  | "tbody"
  | "tfoot"
  | "tr"
  | "th"
  | "td"
  | "col"
  | "colgroup"
  | "details"
  | "summary"
  | "dialog"
  | "template"
  | "slot"
  | "iframe"
  | "noscript"
  | "address"
  | "abbr"
  | "bdi"
  | "bdo"
  | "cite"
  | "data"
  | "del"
  | "dfn"
  | "ins"
  | "kbd"
  | "map"
  | "area"
  | "object"
  | "param"
  | "q"
  | "rp"
  | "rt"
  | "ruby"
  | "s"
  | "samp"
  | "sub"
  | "sup"
  | "u"
  | "var"
  | "wbr"
  | "embed";

type SvgTag =
  | "svg"
  | "g"
  | "path"
  | "circle"
  | "ellipse"
  | "rect"
  | "line"
  | "polyline"
  | "polygon"
  | "text"
  | "tspan"
  | "defs"
  | "symbol"
  | "use"
  | "clipPath"
  | "mask"
  | "linearGradient"
  | "radialGradient"
  | "stop"
  | "desc"
  | "foreignObject"
  | "pattern"
  | "filter"
  | "marker"
  | "image";

/** DOM element corresponding to an owned intrinsic tag. */
export type ElementForTag<Tag extends HtmlTag | SvgTag> = Tag extends HtmlTag
  ? Tag extends keyof HTMLElementTagNameMap
    ? HTMLElementTagNameMap[Tag]
    : HTMLElement
  : Tag extends keyof SVGElementTagNameMap
    ? SVGElementTagNameMap[Tag]
    : SVGElement;

export namespace JSX {
  export type Element = Description;
  export type HTMLAttributes<T extends HTMLElement = HTMLElement> =
    import("./jsx-types.js").HTMLAttributes<T>;
  export type SVGAttributes<T extends SVGElement = SVGElement> =
    import("./jsx-types.js").SVGAttributes<T>;
  export type CSSProperties = import("./jsx-types.js").CSSProperties;
  export type ElementType =
    | string
    | Component<any>
    | typeof import("../island.js").Island
    | typeof import("./description.js").Fragment;
  export type IntrinsicElements = {
    [K in HtmlTag]: K extends "input"
      ? InputProps
      : K extends "textarea"
        ? TextareaProps
        : K extends "select"
          ? SelectProps
          : K extends "iframe"
            ? IframeProps
            : K extends "script" | "style"
              ? RawTextProps<ElementForTag<K>>
              : HTMLAttributes<ElementForTag<K>>;
  } & {
    [K in SvgTag]: SVGAttributes<ElementForTag<K>>;
  } & {
    [K in `${string}-${string}`]: CustomProps;
  };
  export interface IntrinsicAttributes {
    key?: Key | undefined;
  }
  export interface ElementChildrenAttribute {
    children: unknown;
  }
}
