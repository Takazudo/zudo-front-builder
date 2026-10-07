import { Fragment, jsx, jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";

export function PanelSlot({ children }) { return jsx(Fragment, { children }); }
export function createBodyEnd(settings) {
  return function BodyEnd({ children }) {
    if (!settings.enabled) return null;
    return jsxs(Fragment, {
      children: [
        children == null
          ? null
          : jsx("script", {
              id: "factory-feature-shim",
              rawHtml: 'document.documentElement.setAttribute("data-factory-feature-shim", "active");',
            }),
        jsx(PanelSlot, { children }),
      ],
    });
  };
}
export function createChrome(settings) {
  const BodyEnd = createBodyEnd(settings);
  return function Chrome({ children }) {
    return jsx(BodyEnd, { children });
  };
}
