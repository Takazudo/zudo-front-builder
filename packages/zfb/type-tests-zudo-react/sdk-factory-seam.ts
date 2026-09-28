/** Compile the shared SDK calls against the owned JSX factory types. */
import { jsx, Fragment } from "@takazudo/zfb/jsx-factory";
import { Island } from "../src/island.js";
import { getCollection } from "../src/content.js";
import { ClientRouter } from "../../zfb-runtime/src/client-router-component.js";

const child = jsx("p", { children: "owned" });
export const wrapper = Island({ children: child });
export const fragment = jsx(Fragment, { children: child });
export const routerNodes = ClientRouter();
export const contentCall: typeof getCollection = getCollection;
