/** Compile the shared SDK calls against the owned JSX factory types. */
import { jsx, Fragment } from "@takazudo/zfb/jsx-factory";
import { Island } from "../src/island.js";
import { getCollection } from "../src/content.js";
import { ClientRouter } from "../../zfb-runtime/src/client-router-component.js";
import { h, type Child, type Description } from "../src/zudo-react/index.js";

const child = jsx("p", { children: "owned" });
const ownedChild: Child = child;
const ownedElement: Description = h("p", null, "owned");
export const wrapper = Island({ children: child });
export const wrappedElement = Island({ children: ownedElement });
export const acceptedChild = ownedChild;
export const fragment = jsx(Fragment, { children: child });
export const routerNodes = ClientRouter();
export const contentCall: typeof getCollection = getCollection;

// @ts-expect-error the owned island scheduler has no arbitrary activation mode
Island({ when: "eventual", children: child });
