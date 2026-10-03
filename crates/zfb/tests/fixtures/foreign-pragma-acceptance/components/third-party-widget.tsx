"use client";

import { getScope, h, signal, type Ref } from "@takazudo/zfb/zudo-react";

type WidgetInstance = { destroy(): void };
type WidgetModule = { mount(host: Element): WidgetInstance };

const loadWidget = (): Promise<WidgetModule> => import("./widget-adapter");

export function ThirdPartyWidget() {
  const host: Ref<Element> = { current: null };
  const status = signal("Loading widget…");
  const scope = getScope();
  const abortSignal = scope.abortSignal;

  scope.onActivate(() => {
    let instance: WidgetInstance | undefined;
    void loadWidget()
      .then((widget) => {
        if (abortSignal.aborted || !host.current) return;
        instance = widget.mount(host.current);
        status.value = "";
      })
      .catch(() => {
        if (!abortSignal.aborted) status.value = "Widget unavailable.";
      });
    return () => {
      instance?.destroy();
      host.current?.replaceChildren();
    };
  });

  return h("section", null, h("div", { ref: host }), h("p", { role: "status", children: status }));
}
