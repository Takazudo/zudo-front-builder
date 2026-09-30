const words = (value: string) => new Set(value.split(" "));

export const voidTags = words(
  "area base br col embed hr img input link meta param source track wbr",
);
export const booleanAttrs = words(
  "hidden inert readonly autofocus required disabled checked selected multiple open controls muted loop autoplay novalidate formnovalidate allowfullscreen reversed",
);
export const commonAttrs = words(
  "id class title lang dir slot role style hidden inert contenteditable draggable spellcheck tabindex accesskey translate onclick onload onerror",
);
export const htmlAttrs = words(
  "href target rel download src alt width height type name value placeholder for charset datetime readonly autofocus required disabled checked selected multiple open controls muted loop autoplay novalidate formnovalidate maxlength minlength min max step pattern autocomplete accept accept-charset http-equiv content media method action enctype rows cols colspan rowspan scope cite poster loading decoding sizes srcset crossorigin referrerpolicy sandbox allow allowfullscreen start reversed",
);
export const svgAttrs = words(
  "width height viewBox preserveAspectRatio gradientUnits gradientTransform markerWidth markerHeight refX refY xlink:href xml:lang stroke-width fill-rule clip-rule stroke-linecap stroke-linejoin stop-color stop-opacity fill stroke d x y x1 x2 y1 y2 cx cy r rx ry points transform opacity offset",
);
export const svgTags = words(
  "svg g path circle ellipse rect line polyline polygon text tspan defs symbol use clipPath mask linearGradient radialGradient stop title desc foreignObject",
);
export const htmlTags = words(
  "html head body title base link meta style script div span p a br hr main header footer nav section article aside h1 h2 h3 h4 h5 h6 ul ol li dl dt dd blockquote pre code strong em b i small mark time figure figcaption img picture source video audio track canvas form label input button textarea select option optgroup fieldset legend output progress meter datalist table caption thead tbody tfoot tr th td col colgroup details summary dialog template slot iframe noscript address abbr bdi bdo cite data del dfn ins kbd map area object param q rp rt ruby s samp sub sup u var wbr embed xmp noembed noframes plaintext",
);
export type Namespace = "html" | "svg";

type PropAlias = {
  readonly spelling: string;
  readonly namespace?: Namespace;
};

const propAliases: Readonly<Record<string, PropAlias>> = {
  className: { spelling: "class" },
  htmlFor: { spelling: "for" },
  charSet: { spelling: "charset" },
  dateTime: { spelling: "datetime" },
  tabIndex: { spelling: "tabindex" },
  readOnly: { spelling: "readonly" },
  autoComplete: { spelling: "autocomplete" },
  autoFocus: { spelling: "autofocus" },
  maxLength: { spelling: "maxlength" },
  minLength: { spelling: "minlength" },
  crossOrigin: { spelling: "crossorigin" },
  srcSet: { spelling: "srcset" },
  acceptCharset: { spelling: "accept-charset" },
  httpEquiv: { spelling: "http-equiv" },
  encType: { spelling: "enctype" },
  noValidate: { spelling: "novalidate" },
  formNoValidate: { spelling: "formnovalidate" },
  colSpan: { spelling: "colspan" },
  rowSpan: { spelling: "rowspan" },
  referrerPolicy: { spelling: "referrerpolicy" },
  allowFullScreen: { spelling: "allowfullscreen" },
  autoPlay: { spelling: "autoplay" },
  contentEditable: { spelling: "contenteditable" },
  spellCheck: { spelling: "spellcheck" },
  accessKey: { spelling: "accesskey" },
  dangerouslySetInnerHTML: { spelling: "rawHtml" },
  strokeWidth: { spelling: "stroke-width", namespace: "svg" },
  fillRule: { spelling: "fill-rule", namespace: "svg" },
  clipRule: { spelling: "clip-rule", namespace: "svg" },
  strokeLinecap: { spelling: "stroke-linecap", namespace: "svg" },
  strokeLinejoin: { spelling: "stroke-linejoin", namespace: "svg" },
  stopColor: { spelling: "stop-color", namespace: "svg" },
  stopOpacity: { spelling: "stop-opacity", namespace: "svg" },
  xlinkHref: { spelling: "xlink:href", namespace: "svg" },
  xmlLang: { spelling: "xml:lang", namespace: "svg" },
};

const legacyDialect = words(
  "className htmlFor charSet dateTime tabIndex readOnly strokeWidth dangerouslySetInnerHTML",
);

function allowedAliasTarget(spelling: string, namespace: Namespace): boolean {
  if (spelling === "rawHtml") return namespace === "html";
  return (
    commonAttrs.has(spelling) ||
    (namespace === "svg" ? svgAttrs.has(spelling) : htmlAttrs.has(spelling))
  );
}

function eventSuggestion(name: string): string {
  const event = name.slice(2);
  return `on:${event === "DoubleClick" ? "dblclick" : event.toLowerCase()}`;
}

export function isDialectProp(name: string, namespace: Namespace, custom: boolean): boolean {
  if (/^on[A-Z]/.test(name) || legacyDialect.has(name)) return true;
  if (custom) return false;
  const alias = propAliases[name];
  return (
    alias !== undefined &&
    (!alias.namespace || alias.namespace === namespace) &&
    allowedAliasTarget(alias.spelling, namespace)
  );
}

export function dialectSuggestion(
  name: string,
  namespace: Namespace,
  custom: boolean,
): string | undefined {
  if (/^on[A-Z]/.test(name)) return eventSuggestion(name);
  if (custom && !legacyDialect.has(name)) return undefined;
  const alias = propAliases[name];
  if (!alias || (alias.namespace && alias.namespace !== namespace)) return undefined;
  return allowedAliasTarget(alias.spelling, namespace) ? alias.spelling : undefined;
}

export function reactiveModelSuggestion(
  tag: string,
  name: string,
  inputType: string,
  custom: boolean,
): "modelValue" | "modelChecked" | undefined {
  if (custom) return undefined;
  if (name === "checked" && tag === "input") {
    if (inputType === "radio") return "modelValue";
    if (inputType === "checkbox") return "modelChecked";
  }
  if (name === "value") {
    if (tag === "textarea" || tag === "select") return "modelValue";
    if (
      tag === "input" &&
      ["text", "search", "email", "url", "tel", "password"].includes(inputType)
    )
      return "modelValue";
  }
  return undefined;
}
export const formProps = words("modelValue modelChecked defaultValue defaultChecked");
