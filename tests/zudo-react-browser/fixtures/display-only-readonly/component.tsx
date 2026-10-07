import { computed, signal } from "@takazudo/zfb/zudo-react";

let state: {
  source: ReturnType<typeof signal<string>>;
  display: ReturnType<typeof computed<string>>;
};

export default function DisplayOnlyReadonlyProbe() {
  const source = signal("display-only initial");
  const display = computed(() => source.value.toUpperCase());
  state = { source, display };

  return (
    <form id="display-only-form">
      <input id="display-only-input" readonly value={display} />
      <textarea id="display-only-textarea" disabled value={display} />
      <button
        id="display-only-update"
        type="button"
        on:click={() => {
          source.value = "updated by button";
        }}
      >
        Update display
      </button>
    </form>
  );
}

export function run() {
  return state;
}
