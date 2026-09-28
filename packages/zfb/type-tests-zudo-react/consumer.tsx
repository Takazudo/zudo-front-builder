import { h, isDescription, signal, Show, For, type ReadonlySignal } from "@takazudo/zfb/zudo-react";

const name = {
  value: "ok",
  $$zudoReactive: "zudo-react.reactive.v1",
} as const satisfies ReadonlySignal<string>;
const view = (
  <>
    <div
      class="box"
      data-state={true}
      aria-label={name}
      style={{ "font-size": 12, "--accent": "red" }}
    >
      {name}
      <svg viewBox="0 0 10 10">
        <path d="M0 0" stroke-width={2} />
      </svg>
      <my-widget data-label="hi" on:change={() => {}} />
    </div>
    <input type="text" readonly={true} defaultValue="start" />
  </>
);
isDescription(view);
h("div", null, "text");
const visible = signal(true);
const records = signal([{ id: "a" }]);
Show({ when: visible, children: () => <strong>visible</strong>, fallback: () => "hidden" });
For({
  each: records,
  by: (record) => record.id,
  children: (item, index) => (
    <span>
      {item.value.id}
      {index}
    </span>
  ),
});
const structuralJsx = (
  <>
    <Show when={visible}>{() => <strong>visible</strong>}</Show>
    <For each={records} by={(record) => record.id}>
      {(item, index) => (
        <span>
          {item.value.id}
          {index}
        </span>
      )}
    </For>
  </>
);
void structuralJsx;
// @ts-expect-error A structural factory requires a callable child.
Show({ when: visible, children: <strong>invalid</strong> });
// @ts-expect-error A list factory requires item and index signal parameters.
For({ each: records, by: (record) => record.id, children: "invalid" });

// @ts-expect-error Function children are reserved for structural components.
const badChild = <div>{() => "text"}</div>;
// @ts-expect-error React spelling is not an HTML attribute.
const badClass = <div className="box" />;
// @ts-expect-error React spelling is not an HTML attribute.
const badDanger = <div dangerouslySetInnerHTML={{ __html: "x" }} />;
// @ts-expect-error rawHtml accepts trusted strings, not objects.
const badRaw = <div rawHtml={{ __html: "x" }} />;
// @ts-expect-error Object style keys use CSS spelling.
const badStyle = <div style={{ backgroundColor: "red" }} />;
// @ts-expect-error A ref is an object, not a callback.
const badRef = <div ref={() => {}} />;
// @ts-expect-error A text model requires a string-valued signal.
const badModel = <input modelValue={{ value: true, $$zudoReactive: "zudo-react.reactive.v1" }} />;
// @ts-expect-error A div cannot own form model state.
const badModelPlacement = <div modelChecked={name} />;

void [badChild, badClass, badDanger, badRaw, badStyle, badRef, badModel, badModelPlacement];
