import { jsx as _jsx, jsxs as _jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { Fragment as _Fragment } from "@takazudo/zfb/zudo-react/jsx-runtime";
export const headings = [];
function _createMdxContent({ components = {} } = {}) {
    const _components = {
        code: "code",
        li: "li",
        ol: "ol",
        p: "p",
        pre: "pre",
        span: "span",
        ...components
    };
    return /*#__PURE__*/ _jsxs(_Fragment, {
        children: [
            /*#__PURE__*/ _jsx(_components.ol, {
                children: /*#__PURE__*/ _jsx(_components.li, {
                    children: /*#__PURE__*/ _jsx(_components.p, {
                        children: "one"
                    })
                })
            }),
            /*#__PURE__*/ _jsx(_components.pre, {
                class: "syntect-base16-ocean-dark",
                children: /*#__PURE__*/ _jsx(_components.code, {
                    children: /*#__PURE__*/ _jsx(_components.span, {
                        class: "line",
                        children: /*#__PURE__*/ _jsx("span", {
                            rawHtml: "<span style=\"color:#8fa1b3;\">x</span>"
                        })
                    })
                })
            }),
            /*#__PURE__*/ _jsx(_components.ol, {
                start: "2",
                children: /*#__PURE__*/ _jsx(_components.li, {
                    children: /*#__PURE__*/ _jsx(_components.p, {
                        children: "two"
                    })
                })
            })
        ]
    });
}
export default function MDXContent(props = {}) {
    return _createMdxContent(props);
}
