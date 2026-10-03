/** @jsxImportSource preact */
import { render } from "preact";

export function mountLegacyWidget(host) {
  render(<p class="legacy-widget">legacy preact widget</p>, host);
  return {
    destroy() {
      render(null, host);
    },
  };
}
