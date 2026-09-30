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
export const dialect = words(
  "className htmlFor charSet dateTime tabIndex readOnly strokeWidth dangerouslySetInnerHTML",
);
export const formProps = words("modelValue modelChecked defaultValue defaultChecked");
