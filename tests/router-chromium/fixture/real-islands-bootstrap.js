import { init } from "/dist/client-router/router.js";
import { mountIslands } from "@takazudo/zfb/runtime";
import { hydrate, mount } from "/zfb-dist/zudo-react/client.js";
import * as react from "/zfb-dist/zudo-react/index.js";
import { createComponents } from "/real-islands-components.mjs";

const components = createComponents(react);
const stats = (window.__realIslands = { mounts: [], disposals: [] });
const manifest = Object.fromEntries(
  Object.entries(components).map(([name, component]) => [
    name,
    {
      identity: { component: name, build: "browser-fixture-1" },
      mount(props, element, mode) {
        const handle = (mode === "hydrate" ? hydrate : mount)(react.h(component, props), element, {
          identity: { component: name, build: "browser-fixture-1" },
        });
        if (!handle) return null;
        stats.mounts.push({ name, id: props.id ?? name, mode, element });
        return {
          unmount: () => handle.unmount(),
          dispose: () => {
            stats.disposals.push({ name, id: props.id ?? name, element });
            handle.dispose();
          },
        };
      },
    },
  ]),
);
mountIslands(manifest);
init();
