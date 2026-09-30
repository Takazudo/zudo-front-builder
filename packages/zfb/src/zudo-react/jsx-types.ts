import type { Child, Component, Key } from "./description.js";
import type { Listener, Ref } from "./index.js";
import type { ReadonlySignal, Signal } from "./reactive-types.js";

type Value<T> = T | ReadonlySignal<T> | undefined;
type ScalarAttribute = Value<string | number | boolean | null>;
type DimensionAttribute = Value<string | number | null>;
type StringAttribute = Value<string | null>;
type BooleanAttribute = Value<boolean | null>;
type CssProperty =
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
type CssStyle = Readonly<Partial<Record<CssProperty, string | number | null | undefined>>>;
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
  style?: Value<string | CssStyle | null>;
  hidden?: BooleanAttribute;
  inert?: BooleanAttribute;
  contenteditable?: StringAttribute;
  draggable?: StringAttribute;
  spellcheck?: StringAttribute;
  tabindex?: Value<number | null>;
  accesskey?: StringAttribute;
  translate?: StringAttribute;
  onclick?: string | undefined;
  onload?: string | undefined;
  onerror?: string | undefined;
}

interface HtmlAttributes<T extends Element = HTMLElement> extends CommonAttributes<T> {
  href?: StringAttribute;
  target?: StringAttribute;
  rel?: StringAttribute;
  download?: StringAttribute;
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

type HtmlProps<
  TRef extends Element = HTMLElement,
  TEvent extends HTMLElement = HTMLElement,
> = HtmlAttributes<TRef> & DataAria & EventProps<TEvent, HTMLElementEventMap>;
type InputBase = Omit<HtmlAttributes<HTMLInputElement>, "type"> &
  DataAria &
  EventProps<HTMLInputElement, HTMLElementEventMap> & {
    defaultValue?: string | undefined;
    defaultChecked?: boolean | undefined;
  };
type InputProps = InputBase &
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
type TextareaProps = HtmlProps<HTMLTextAreaElement, HTMLTextAreaElement> & {
  modelValue?: Signal<string> | undefined;
  defaultValue?: string | undefined;
};
type SelectProps = HtmlProps<HTMLSelectElement, HTMLSelectElement> & {
  modelValue?: Signal<string> | undefined;
  defaultValue?: string | undefined;
};

interface SvgAttributes extends CommonAttributes<SVGElement> {
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
  "stroke-width"?: ScalarAttribute;
  "fill-rule"?: StringAttribute;
  "clip-rule"?: StringAttribute;
  "stroke-linecap"?: StringAttribute;
  "stroke-linejoin"?: StringAttribute;
  "stop-color"?: StringAttribute;
  "stop-opacity"?: ScalarAttribute;
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

type SvgProps<TEvent extends SVGElement = SVGElement> = SvgAttributes &
  DataAria &
  EventProps<TEvent, SVGElementEventMap>;
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
  | "foreignObject";

export namespace JSX {
  export type Element = Child;
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
          : HtmlProps<
              HTMLElement,
              K extends keyof HTMLElementTagNameMap ? HTMLElementTagNameMap[K] : HTMLElement
            >;
  } & {
    [K in SvgTag]: SvgProps<
      K extends keyof SVGElementTagNameMap ? SVGElementTagNameMap[K] : SVGElement
    >;
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
