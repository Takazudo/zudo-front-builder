import { signal } from "@takazudo/zfb/zudo-react";

let hidden: ReturnType<typeof signal<boolean>>;

export default function HiddenPanelProbe() {
  hidden = signal(true);
  return (
    <section>
      <button
        id="show-panel"
        on:click={() => {
          hidden.value = false;
        }}
      >
        Show panel
      </button>
      <div class="zudo-tab-panel" hidden={hidden} id="tab-panel">
        Active tab content
      </div>
    </section>
  );
}

export function run() {
  return { hidden };
}
