export const strings = {
  index: {
    title: "Utility reference",
    description: "Browse the closed zudo-wind v1 utility catalog by family and declaration.",
    intro:
      "Choose a utility family by the result you want. Start with layout and spacing, then typography, surfaces, motion or interaction. Every family retains its supported values, declarations and advanced catalog reference.",
    specVersionLabel: "Catalog spec version",
    specRevisionLabel: "Catalog spec revision",
    familyColumn: "Family",
    entryCountColumn: "Catalog entries",
  },
  labels: {
    field: "Field",
    value: "Value",
    catalogIdentifier: "Catalog identifier",
    classPattern: "Class pattern",
    grammar: "Grammar",
    acceptedValues: "Accepted values",
    tokenCategories: "Token categories",
    negativePolicy: "Negative-value policy",
    conflictGroup: "Conflict group and rank",
    orderRank: "Order rank",
    selectorShape: "Selector shape",
    specVersion: "Spec version",
    emitterProperties: "Emitter properties",
    noAcceptedValues: "The catalog has no separate accepted-value records for this entry.",
    noTokenCategories: "No token category",
    noDeclarations: "No declaration records",
    noExamples: "The catalog has no examples for this entry.",
  },
  headings: {
    acceptedValues: "Accepted values",
    declarationTemplates: "Declaration templates",
    examples: "Catalog examples",
    relatedGuides: "Related guides",
  },
  columns: {
    kind: "Kind",
    suffix: "Suffix",
    emittedValue: "Emitted value",
    property: "Property",
    valueKind: "Value kind",
    fixedValue: "Fixed value",
    candidate: "Example class",
    expectedDeclarations: "Expected declarations",
  },
  links: {
    utilityGrammar: "Utility grammar",
    variants: "Variants",
  },
  reader: {
    lookup: "Quick reference",
    setup: "Example setup",
    workedExamples: "Examples in use",
    customValues: "Custom and named values",
    technical: "Catalog details",
    config: "Configuration for this example",
    scaffold: "Authored demonstration CSS",
    diagnostics: "Expected diagnostics",
    taskColumn: "Use it to",
    browse: "Choose by task",
  },
  families: {
    svg: {
      title: "SVG paint",
      description: "Set currentColor or none for SVG fill and stroke.",
    },
    display: {
      title: "Display",
      description: "Choose whether an element is hidden, inline, block-level, flex, or grid.",
    },
    position: {
      title: "Position",
      description: "Set the positioning mode used to place an element in its containing block.",
    },
    inset: {
      title: "Inset",
      description: "Set physical offsets with spacing values, fractions, auto, or full.",
    },
    "flex-container": {
      title: "Flex container",
      description: "Set a flex container's main direction and wrapping behavior.",
    },
    "flex-item": {
      title: "Flex item",
      description: "Set flex growth, shrinkage, and the shorthand sizing behavior of an item.",
    },
    grid: {
      title: "Grid",
      description: "Define grid tracks and place items by row or column span and line.",
    },
    alignment: {
      title: "Alignment",
      description: "Align items, individual items, and grid content along supported axes.",
    },
    sizing: {
      title: "Sizing",
      description:
        "Set width and height constraints using spacing, size tokens, fractions, and keywords.",
    },
    padding: {
      title: "Padding",
      description: "Set physical inner spacing on every side, an axis, or an individual edge.",
    },
    margin: {
      title: "Margin",
      description:
        "Set physical outer spacing, including supported negative values and auto margins.",
    },
    gap: {
      title: "Gap",
      description: "Set row and column gaps between items in a layout container.",
    },
    "child-space": {
      title: "Child spacing",
      description: "Add spacing between later visible siblings with direct child selectors.",
    },
    overflow: {
      title: "Overflow",
      description: "Control content overflow and overscroll behavior on an element or axis.",
    },
    "z-index": {
      title: "Z index",
      description:
        "Set stacking order with a bounded integer, an auto value, or a configured token.",
    },
    "font-family": {
      title: "Font family",
      description: "Select a configured font family or provide an arbitrary font-family value.",
    },
    "font-weight": {
      title: "Font weight",
      description: "Select a configured font weight or provide an arbitrary weight value.",
    },
    "font-size": {
      title: "Font size",
      description: "Set font size from a configured token or an arbitrary CSS font-size value.",
    },
    "line-height": {
      title: "Line height",
      description:
        "Set line height from a configured token or an arbitrary CSS value; leading-none falls back to 1 when no none token is configured.",
    },
    tracking: {
      title: "Letter spacing",
      description:
        "Set letter spacing with a configured token or arbitrary value; negatives are allowed.",
    },
    "text-layout": {
      title: "Text layout",
      description: "Set text alignment, whitespace handling, wrapping, and truncation behavior.",
    },
    "text-style": {
      title: "Text style",
      description: "Set text decoration, letter case, font style, numeric shaping, and smoothing.",
    },
    color: {
      title: "Text color",
      description: "Set text color from configured color values or a validated arbitrary color.",
    },
    background: {
      title: "Background color",
      description: "Set a background color from configured values or a validated arbitrary color.",
    },
    "border-width": {
      title: "Border width",
      description: "Set physical border widths in pixels and establish a solid border style.",
    },
    "border-color": {
      title: "Border color",
      description:
        "Set physical border colors from configured values or a validated arbitrary color.",
    },
    "border-style": {
      title: "Border style",
      description: "Choose a supported border line style or border-collapse behavior.",
    },
    radius: {
      title: "Border radius",
      description:
        "Set all corners, edge corners, or a single corner with configured radius values.",
    },
    "divide-width": {
      title: "Divide width",
      description: "Set solid border widths on later visible siblings along an axis.",
    },
    "divide-color": {
      title: "Divide color",
      description: "Set the border color of later visible siblings between grouped items.",
    },
    "outline-width": {
      title: "Outline width and offset",
      description:
        "Set outline width or offset with length values; negative values are allowed only for offset.",
    },
    "outline-color": {
      title: "Outline color",
      description: "Set outline color from configured values or a validated arbitrary color.",
    },
    "outline-style": {
      title: "Outline style",
      description: "Choose a supported outline line style, including none.",
    },
    shadow: {
      title: "Shadow",
      description: "Set a configured box shadow or remove the box shadow.",
    },
    opacity: {
      title: "Opacity",
      description: "Set element opacity as an integer percentage or validated arbitrary value.",
    },
    transition: {
      title: "Transition",
      description: "Choose the property set used by a transition and its built-in timing defaults.",
    },
    duration: {
      title: "Transition duration",
      description: "Set transition duration with a bounded millisecond value or arbitrary time.",
    },
    easing: {
      title: "Easing",
      description:
        "Set transition timing with a configured easing token or arbitrary timing function.",
    },
    translate: {
      title: "Translate",
      description:
        "Translate an element along an axis with spacing, full, fractions, or negative values.",
    },
    rotate: {
      title: "Rotation",
      description: "Rotate an element by a supported degree value or arbitrary angle.",
    },
    interaction: {
      title: "Interaction",
      description: "Set cursor, pointer, selection, resizing, and accent-color behavior.",
    },
    list: {
      title: "List style",
      description: "Set list markers and whether markers appear inside or outside the content box.",
    },
    aspect: {
      title: "Aspect ratio",
      description: "Set an automatic, named, or arbitrary aspect ratio.",
    },
    "scroll-margin": {
      title: "Scroll margin",
      description: "Set physical scroll margins with spacing values, including negative values.",
    },
    miscellaneous: {
      title: "Miscellaneous",
      description:
        "Set vertical alignment, box sizing, object fitting, and screen-reader-only clipping.",
    },
    "text-decoration-color": {
      title: "Text decoration",
      description: "Set text-decoration color, thickness, or style with the catalog values.",
    },
    "text-underline-offset": {
      title: "Underline offset",
      description:
        "Set the underline offset in whole pixels or as an arbitrary nonnegative length.",
    },
    visibility: {
      title: "Visibility",
      description: "Show or hide an element while it keeps its place in the layout.",
    },
  },
};

export default strings;
