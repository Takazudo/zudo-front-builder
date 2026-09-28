import { For, getScope, Show, signal } from "@takazudo/zfb/zudo-react";

type Item = { id: string; label: string };
type State = {
  shown: ReturnType<typeof signal<boolean>>;
  items: ReturnType<typeof signal<Item[]>>;
  branchActivations: number;
  branchDisposals: number;
  itemActivations: string[];
  itemDisposals: string[];
};
let state: State;

export default function StructureProbe() {
  state = {
    shown: signal(true),
    items: signal<Item[]>([
      { id: "a", label: "Alpha" },
      { id: "b", label: "Beta" },
    ]),
    branchActivations: 0,
    branchDisposals: 0,
    itemActivations: [],
    itemDisposals: [],
  };

  function BranchProbe() {
    getScope().onActivate(() => {
      state.branchActivations++;
    });
    getScope().onCleanup(() => {
      state.branchDisposals++;
    });
    return <strong id="conditional-branch">Visible branch</strong>;
  }

  return (
    <section>
      <Show when={state.shown}>{() => <BranchProbe />}</Show>
      <For each={state.items} by={(item) => item.id}>
        {(item, index) => {
          const initialId = item.value.id;
          getScope().onActivate(() => {
            state.itemActivations.push(initialId);
          });
          getScope().onCleanup(() => {
            state.itemDisposals.push(initialId);
          });
          return (
            <div class="list-item" data-item-id={item.value.id}>
              <span>{item.value.label}</span>
              <small>{index}</small>
              <input id={`item-input-${initialId}`} defaultValue={item.value.label} />
            </div>
          );
        }}
      </For>
    </section>
  );
}

export function run() {
  return state;
}
