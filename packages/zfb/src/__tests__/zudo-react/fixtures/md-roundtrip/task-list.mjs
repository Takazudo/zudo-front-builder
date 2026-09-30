import { jsx as _jsx, jsxs as _jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { Fragment as _Fragment } from "@takazudo/zfb/zudo-react/jsx-runtime";
export const headings = [];
function _createMdxContent({ components = {} } = {}) {
    const _components = {
        input: "input",
        li: "li",
        p: "p",
        ul: "ul",
        ...components
    };
    return /*#__PURE__*/ _jsx(_Fragment, {
        children: /*#__PURE__*/ _jsxs(_components.ul, {
            children: [
                /*#__PURE__*/ _jsx(_components.li, {
                    children: /*#__PURE__*/ _jsxs(_components.p, {
                        children: [
                            /*#__PURE__*/ _jsx(_components.input, {
                                type: "checkbox",
                                disabled: true,
                                checked: true
                            }),
                            " ",
                            "done"
                        ]
                    })
                }),
                /*#__PURE__*/ _jsx(_components.li, {
                    children: /*#__PURE__*/ _jsxs(_components.p, {
                        children: [
                            /*#__PURE__*/ _jsx(_components.input, {
                                type: "checkbox",
                                disabled: true
                            }),
                            " ",
                            "todo"
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
