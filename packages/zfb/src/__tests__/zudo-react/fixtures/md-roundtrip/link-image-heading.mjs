import { jsx as _jsx, jsxs as _jsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { Fragment as _Fragment } from "@takazudo/zfb/zudo-react/jsx-runtime";
export const headings = [
    {
        depth: 1,
        slug: "title",
        text: "Title"
    }
];
function _createMdxContent({ components = {} } = {}) {
    const _components = {
        a: "a",
        h1: "h1",
        img: "img",
        p: "p",
        ...components
    };
    return /*#__PURE__*/ _jsxs(_Fragment, {
        children: [
            /*#__PURE__*/ _jsx(_components.h1, {
                children: "Title"
            }),
            /*#__PURE__*/ _jsx(_components.p, {
                children: /*#__PURE__*/ _jsx(_components.a, {
                    href: "/x",
                    title: "t",
                    children: "a"
                })
            }),
            /*#__PURE__*/ _jsx(_components.p, {
                children: /*#__PURE__*/ _jsx(_components.img, {
                    src: "/i.png",
                    alt: "alt",
                    title: "t"
                })
            })
        ]
    });
}
export default function MDXContent(props = {}) {
    return _createMdxContent(props);
}
