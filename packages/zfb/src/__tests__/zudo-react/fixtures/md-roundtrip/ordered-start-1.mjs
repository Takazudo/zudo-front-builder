import { jsx as _jsx, jsxs as _jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { Fragment as _Fragment } from "@takazudo/zfb/zudo-react/jsx-runtime";
export const headings = [];
function _createMdxContent({ components = {} } = {}) {
    const _components = {
        li: "li",
        ol: "ol",
        p: "p",
        ...components
    };
    return /*#__PURE__*/ _jsx(_Fragment, {
        children: /*#__PURE__*/ _jsxs(_components.ol, {
            children: [
                /*#__PURE__*/ _jsx(_components.li, {
                    children: /*#__PURE__*/ _jsx(_components.p, {
                        children: "one"
                    })
                }),
                /*#__PURE__*/ _jsx(_components.li, {
                    children: /*#__PURE__*/ _jsx(_components.p, {
                        children: "two"
                    })
                })
            ]
        })
    });
}
export default function MDXContent(props = {}) {
    return _createMdxContent(props);
}
