import { jsx as _jsx, jsxs as _jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { Fragment as _Fragment } from "@takazudo/zfb/zudo-react/jsx-runtime";
export const headings = [];
function _createMdxContent({ components = {} } = {}) {
    const _components = {
        a: "a",
        div: "div",
        li: "li",
        ol: "ol",
        p: "p",
        section: "section",
        sup: "sup",
        ...components
    };
    return /*#__PURE__*/ _jsxs(_Fragment, {
        children: [
            /*#__PURE__*/ _jsxs(_components.p, {
                children: [
                    "Here is a footnote.",
                    /*#__PURE__*/ _jsx(_components.sup, {
                        children: /*#__PURE__*/ _jsx(_components.a, {
                            href: "#user-content-fn-1",
                            id: "user-content-fnref-1",
                            "data-footnote-ref": "",
                            "aria-describedby": "footnote-label",
                            children: "1"
                        })
                    })
                ]
            }),
            /*#__PURE__*/ _jsxs(_components.section, {
                "data-footnotes": "",
                class: "footnotes",
                children: [
                    /*#__PURE__*/ _jsx(_components.div, {
                        role: "heading",
                        "aria-level": "2",
                        class: "sr-only",
                        style: "position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);clip-path:inset(50%);white-space:nowrap;border:0",
                        id: "footnote-label",
                        children: "Footnotes"
                    }),
                    /*#__PURE__*/ _jsx(_components.ol, {
                        children: /*#__PURE__*/ _jsxs(_components.li, {
                            id: "user-content-fn-1",
                            children: [
                                /*#__PURE__*/ _jsx(_components.p, {
                                    children: "Footnote text."
                                }),
                                /*#__PURE__*/ _jsx(_components.a, {
                                    href: "#user-content-fnref-1",
                                    "data-footnote-backref": "",
                                    "aria-label": "Back to reference 1",
                                    children: "↩"
                                })
                            ]
                        })
                    })
                ]
            })
        ]
    });
}
export default function MDXContent(props = {}) {
    return _createMdxContent(props);
}
