// The documented adapter: the widget package owns its framework and DOM host.
import { mountLegacyWidget } from "@fixture/legacy-widget";

export function mount(host: Element): { destroy(): void } {
  return mountLegacyWidget(host);
}
