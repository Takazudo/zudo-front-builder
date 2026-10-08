import { h, signal, Show, flush } from "@takazudo/zfb/zudo-react";
import { mount, hydrate } from "@takazudo/zfb/zudo-react/client";
import { islandRoot, renderToString } from "@takazudo/zfb/zudo-react/server";

window.verifyLateShow = async () => {
  const results = [];
  for (const attach of [mount, hydrate]) {
    for (const wrapped of [false, true]) {
      const visible = signal(false);
      const identity = { component: "Demo", build: "published-verification" };
      const diagnostics = [];
      function Demo() {
        const branch = Show({ when: visible, children: () => h("p", null, "later") });
        return wrapped ? h("section", null, branch) : branch;
      }
      const host = document.createElement("div");
      document.body.append(host);
      host.innerHTML = renderToString(islandRoot(h(Demo, {}), { identity }));
      const container = host.firstElementChild;
      const handle = attach(h(Demo, {}), container, {
        identity,
        report: (d) => diagnostics.push(d),
      });
      if (!handle || container.querySelector("p")) throw new Error("invalid initially false root");
      visible.value = true;
      await flush();
      if (container.querySelector("p")?.textContent !== "later")
        throw new Error("branch never rendered");
      const rendered = container.innerHTML;
      handle.unmount();
      const after = container.innerHTML;
      const childNodes = container.childNodes.length;
      const disposed = handle.disposed;
      handle.unmount();
      visible.value = false;
      await flush();
      if (container.innerHTML !== after) throw new Error("disposed root still updates");
      results.push({
        attach: attach.name,
        wrapped,
        rendered,
        after,
        childNodes,
        disposed,
        diagnostics,
      });
      host.remove();
    }
  }
  return results;
};
