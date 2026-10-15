import { jsx as _jsx, jsxs as _jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { Fragment as _Fragment } from "@takazudo/zfb/zudo-react/jsx-runtime";
export const headings = [];
function _createMdxContent({ components = {} } = {}) {
    const _components = {
        p: "p",
        span: "span",
        strong: "strong",
        table: "table",
        tbody: "tbody",
        td: "td",
        th: "th",
        thead: "thead",
        tr: "tr",
        ...components
    };
    return /*#__PURE__*/ _jsxs(_Fragment, {
        children: [
            /*#__PURE__*/ _jsxs(_components.table, {
                children: [
                    /*#__PURE__*/ _jsx(_components.thead, {
                        children: /*#__PURE__*/ _jsx(_components.tr, {
                            children: /*#__PURE__*/ _jsx(_components.th, {
                                children: "A"
                            })
                        })
                    }),
                    /*#__PURE__*/ _jsx(_components.tbody, {
                        children: /*#__PURE__*/ _jsx(_components.tr, {
                            children: /*#__PURE__*/ _jsx(_components.td, {
                                children: "B"
                            })
                        })
                    })
                ]
            }),
            /*#__PURE__*/ _jsxs(_components.p, {
                children: [
                    "Before ",
                    /*#__PURE__*/ _jsx(_components.strong, {
                        children: "bold"
                    }),
                    " ",
                    /*#__PURE__*/ _jsx(_components.span, {
                        children: "inline"
                    }),
                    ".\n"
                ]
            }),
            /*#__PURE__*/ _jsx(_components.table, {
                class: "compact",
                children: /*#__PURE__*/ _jsx(_components.tbody, {
                    children: /*#__PURE__*/ _jsx(_components.tr, {
                        children: /*#__PURE__*/ _jsx(_components.td, {
                            children: "C"
                        })
                    })
                })
            }),
            /*#__PURE__*/ _jsx(_components.p, {
                children: "\nAfter."
            })
        ]
    });
}
export default function MDXContent(props = {}) {
    return _createMdxContent(props);
}
