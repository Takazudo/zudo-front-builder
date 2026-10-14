import { jsx as _jsx, jsxs as _jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { Fragment as _Fragment } from "@takazudo/zfb/zudo-react/jsx-runtime";
export const headings = [];
function _createMdxContent({ components = {} } = {}) {
    const _components = {
        caption: "caption",
        col: "col",
        colgroup: "colgroup",
        p: "p",
        strong: "strong",
        table: "table",
        tbody: "tbody",
        td: "td",
        tfoot: "tfoot",
        th: "th",
        thead: "thead",
        tr: "tr",
        ...components
    };
    return /*#__PURE__*/ _jsxs(_Fragment, {
        children: [
            /*#__PURE__*/ _jsx(_components.p, {
                children: "Before the table."
            }),
            /*#__PURE__*/ _jsxs(_components.table, {
                children: [
                    /*#__PURE__*/ _jsx(_components.caption, {
                        children: "Scores"
                    }),
                    /*#__PURE__*/ _jsxs(_components.colgroup, {
                        children: [
                            /*#__PURE__*/ _jsx(_components.col, {}),
                            /*#__PURE__*/ _jsx(_components.col, {})
                        ]
                    }),
                    /*#__PURE__*/ _jsx(_components.thead, {
                        children: /*#__PURE__*/ _jsxs(_components.tr, {
                            children: [
                                /*#__PURE__*/ _jsx(_components.th, {
                                    children: "Name"
                                }),
                                /*#__PURE__*/ _jsx(_components.th, {
                                    children: "Score"
                                })
                            ]
                        })
                    }),
                    /*#__PURE__*/ _jsxs(_components.tbody, {
                        children: [
                            /*#__PURE__*/ _jsxs(_components.tr, {
                                children: [
                                    /*#__PURE__*/ _jsx(_components.td, {
                                        children: "Ann"
                                    }),
                                    /*#__PURE__*/ _jsx(_components.td, {
                                        children: /*#__PURE__*/ _jsx(_components.strong, {
                                            children: "10"
                                        })
                                    })
                                ]
                            }),
                            /*#__PURE__*/ _jsxs(_components.tr, {
                                children: [
                                    /*#__PURE__*/ _jsx(_components.td, {
                                        children: "Bob"
                                    }),
                                    /*#__PURE__*/ _jsx(_components.td, {
                                        children: "9"
                                    })
                                ]
                            }),
                            /*#__PURE__*/ _jsxs(_components.tr, {
                                children: [
                                    /*#__PURE__*/ _jsx(_components.td, {
                                        children: "Cy"
                                    }),
                                    /*#__PURE__*/ _jsx(_components.td, {
                                        children: /*#__PURE__*/ _jsx(_components.p, {
                                            children: "Cell paragraph."
                                        })
                                    })
                                ]
                            })
                        ]
                    }),
                    /*#__PURE__*/ _jsx(_components.tfoot, {
                        children: /*#__PURE__*/ _jsxs(_components.tr, {
                            children: [
                                /*#__PURE__*/ _jsx(_components.td, {
                                    children: "Total"
                                }),
                                /*#__PURE__*/ _jsx(_components.td, {
                                    children: "19"
                                })
                            ]
                        })
                    })
                ]
            }),
            /*#__PURE__*/ _jsx(_components.p, {
                children: "After the table."
            })
        ]
    });
}
export default function MDXContent(props = {}) {
    return _createMdxContent(props);
}
