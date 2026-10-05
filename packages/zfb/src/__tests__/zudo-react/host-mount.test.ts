import { beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { h, type Child, type Diagnostic } from "../../zudo-react/index.js";
import { mount } from "../../zudo-react/client.js";

function HostWidget(props: { onPick: () => void; slot: Child }) {
  return h(
    "section",
    null,
    h("button", { "on:click": props.onPick, children: "Pick" }),
    props.slot,
  );
}

const identity = { component: HostWidget.name, build: "preview-frame-v1" };

function callerOwnedHost(): HTMLDivElement {
  const host = document.createElement("div");
  host.setAttribute("data-zfb-island-skip-ssr", identity.component);
  host.setAttribute("data-zfb-transport", "json/1");
  host.setAttribute("data-zfb-protocol", "zudo-react/1");
  host.setAttribute("data-zfb-build", identity.build);
  return host;
}

beforeEach(() => {
  document.body.replaceChildren();
});

describe("caller-owned mount", () => {
  it("mounts callback and slot props, then disposes and remounts on the same host", () => {
    const diagnostics: Diagnostic[] = [];
    const onPick = vi.fn();
    const props = { onPick, slot: h("strong", { children: "Provided slot" }) };
    const host = callerOwnedHost();
    document.body.append(host);

    const mountWidget = () =>
      mount(h(HostWidget, props), host, {
        identity,
        report(diagnostic) {
          diagnostics.push(diagnostic);
        },
      });

    const first = mountWidget();
    expect(first).not.toBeNull();
    expect(host.querySelector("strong")?.textContent).toBe("Provided slot");
    const firstButton = host.querySelector("button")!;
    firstButton.click();
    expect(onPick).toHaveBeenCalledTimes(1);

    first!.dispose();
    expect(first!.disposed).toBe(true);
    firstButton.click();
    expect(onPick).toHaveBeenCalledTimes(1);

    const second = mountWidget();
    expect(second).not.toBeNull();
    expect(host.querySelector("button")).not.toBe(firstButton);
    expect(host.querySelector("strong")?.textContent).toBe("Provided slot");
    host.querySelector("button")!.click();
    expect(onPick).toHaveBeenCalledTimes(2);
    expect(diagnostics).toEqual([]);

    second!.unmount();
    expect(host.childNodes).toHaveLength(0);
  });

  it("reports identity failures and returns null", () => {
    const diagnostics: Diagnostic[] = [];
    const host = callerOwnedHost();
    document.body.append(host);
    host.removeAttribute("data-zfb-transport");

    const root = mount(h(HostWidget, { onPick: vi.fn(), slot: "slot" }), host, {
      identity,
      report(diagnostic) {
        diagnostics.push(diagnostic);
      },
    });

    expect(root).toBeNull();
    expect(diagnostics).toHaveLength(1);
    expect(diagnostics[0]).toMatchObject({ code: "ZR_IDENTITY", phase: "preflight" });
    expect(host.childNodes).toHaveLength(0);
  });
});
