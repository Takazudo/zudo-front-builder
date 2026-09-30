import { jsx as _jsx, jsxs as _jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { Fragment as _Fragment } from "@takazudo/zfb/zudo-react/jsx-runtime";
export const headings = [];
function _createMdxContent({ components = {} } = {}) {
    const _components = {
        table: "table",
        tbody: "tbody",
        td: "td",
        th: "th",
        thead: "thead",
        tr: "tr",
        ...components
    };
    return /*#__PURE__*/ _jsx(_Fragment, {
        children: /*#__PURE__*/ _jsxs(_components.table, {
            children: [
                /*#__PURE__*/ _jsx(_components.thead, {
                    children: /*#__PURE__*/ _jsxs(_components.tr, {
                        children: [
                            /*#__PURE__*/ _jsx(_components.th, {
                                style: "text-align: left",
                                children: "Left"
                            }),
                            /*#__PURE__*/ _jsx(_components.th, {
                                style: "text-align: center",
                                children: "Center"
                            }),
                            /*#__PURE__*/ _jsx(_components.th, {
                                style: "text-align: right",
                                children: "Right"
                            }),
                            /*#__PURE__*/ _jsx(_components.th, {
                                children: "Plain"
                            })
                        ]
                    })
                }),
                /*#__PURE__*/ _jsx(_components.tbody, {
                    children: /*#__PURE__*/ _jsxs(_components.tr, {
                        children: [
                            /*#__PURE__*/ _jsx(_components.td, {
                                style: "text-align: left",
                                children: "a"
                            }),
                            /*#__PURE__*/ _jsx(_components.td, {
                                style: "text-align: center",
                                children: "b"
                            }),
                            /*#__PURE__*/ _jsx(_components.td, {
                                style: "text-align: right",
                                children: "c"
                            }),
                            /*#__PURE__*/ _jsx(_components.td, {
                                children: "d"
                            })
                        ]
                    })
                })
            ]
        })
    });
}
export default function MDXContent(props = {}) {
    return _createMdxContent(props);
}
