import {
  h,
  isDescription,
  signal,
  Show,
  For,
  type Description,
  type ReadonlySignal,
} from "@takazudo/zfb/zudo-react";
import { Island } from "@takazudo/zfb";
import {
  islandRoot,
  renderToString,
  serializeProps,
  type IslandOptions,
  type RenderOptions,
} from "@takazudo/zfb/zudo-react/server";
// @ts-expect-error The internal island marker is not part of the public server API.
import { islandRootType } from "@takazudo/zfb/zudo-react/server";

const serverRenderOptions: RenderOptions = {};
const serverIslandOptions: IslandOptions = { identity: { component: "Example", build: "b1" } };
void [islandRoot, renderToString, serializeProps, serverRenderOptions, serverIslandOptions];
void islandRootType;

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
        <svg width="16" height="16" />
        <path d="M0 0" stroke-width={2} />
      </svg>
      <my-widget data-label="hi" on:change={() => {}} />
    </div>
    <input type="text" readonly={true} defaultValue="start" />
  </>
);
const orderedLists = (
  <>
    <ol start={3} reversed />
    <ol start="2" />
  </>
);
// @ts-expect-error Ordered list start does not accept booleans.
const badOrderedListStart = <ol start={true} />;
void [orderedLists, badOrderedListStart];
isDescription(view);
const island = (
  <Island>
    <ExampleIsland />
  </Island>
);
function ExampleIsland() {
  return <span>example</span>;
}
void island;
// Island now returns an owned branded description directly.
const assumedOwned: Description = Island({ children: <ExampleIsland /> });
void assumedOwned;
// @ts-expect-error SVG dimensions do not accept booleans.
const badSvgWidth = <svg width={true} />;
// @ts-expect-error Reactive SVG dimensions do not accept booleans.
const badSvgHeight = <svg height={signal(true)} />;
void [badSvgWidth, badSvgHeight];
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
const unbrandedVNode = { type: "span", props: {}, key: null };
// @ts-expect-error Ordinary unbranded vnode objects are not owned JSX children.
const badUnbrandedChild = <div>{unbrandedVNode}</div>;
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

void [
  badChild,
  badUnbrandedChild,
  badClass,
  badDanger,
  badRaw,
  badStyle,
  badRef,
  badModel,
  badModelPlacement,
];
