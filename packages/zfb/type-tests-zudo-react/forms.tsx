import { signal, computed, type ReadonlySignal } from "@takazudo/zfb/zudo-react";

const text = signal("text");
const checked = signal(false);
const chosen = signal<string | null>(null);
const readonlyText: ReadonlySignal<string> = text;
const readonlyChecked: ReadonlySignal<boolean> = checked;
const readonlyChosen: ReadonlySignal<string | null> = chosen;
export const forms = [
  <input type="text" value="a" />,
  <input type="checkbox" checked={true} />,
  <input type="checkbox" checked={null} />,
  <option selected={checked}>Selected reactively</option>,
  <my-widget value="a" checked={null} />,
  <input modelValue={text} />,
  <textarea modelValue={text} />,
  <select modelValue={text}>
    <option value="text">Text</option>
  </select>,
  <input type="checkbox" modelChecked={checked} />,
  <input type="radio" name="group" value="text" modelValue={chosen} />,
  // @ts-expect-error Radio checked does not accept reactive values.
  <input type="radio" name="g" value="a" checked={computed(() => true)} />,
  // @ts-expect-error HTML value attributes are static strings or numbers.
  <input type="text" value={signal("a")} />,
  // @ts-expect-error Textarea value attributes are static strings or numbers.
  <textarea value={signal("a")} />,
  // @ts-expect-error Select value attributes are static strings or numbers.
  <select value={signal("a")}>
    <option value="a">A</option>
  </select>,
  // @ts-expect-error Button value attributes are static strings or numbers.
  <button value={signal("a")} />,
  // @ts-expect-error Option value attributes are static strings or numbers.
  <option value={signal("a")} />,
  // @ts-expect-error Checkbox checked attributes are static booleans.
  <input type="checkbox" checked={signal(true)} />,
  // @ts-expect-error Custom element values are static strings.
  <my-widget value={signal("a")} />,
  // @ts-expect-error Custom element checked values must be nullish.
  <my-widget checked={true} />,
  // @ts-expect-error A computed is readonly, even for text input.
  <input modelValue={computed(() => "x")} />,
  // @ts-expect-error A readonly view is not a writable text model.
  <input modelValue={readonlyText} />,
  // @ts-expect-error A computed is readonly for textarea.
  <textarea modelValue={computed(() => "x")} />,
  // @ts-expect-error A computed is readonly for select.
  <select modelValue={computed(() => "x")} />,
  // @ts-expect-error A computed is readonly for checkbox.
  <input type="checkbox" modelChecked={computed(() => true)} />,
  // @ts-expect-error A readonly view is not a writable checkbox model.
  <input type="checkbox" modelChecked={readonlyChecked} />,
  // @ts-expect-error A computed is readonly for radio.
  <input type="radio" name="group" value="x" modelValue={computed<string | null>(() => "x")} />,
  // @ts-expect-error A readonly view is not a writable radio model.
  <input type="radio" name="group" value="x" modelValue={readonlyChosen} />,
  // @ts-expect-error File inputs cannot have models.
  <input type="file" modelValue={text} />,
  // @ts-expect-error Checkbox models are boolean.
  <input type="checkbox" modelChecked={text} />,
  // @ts-expect-error Model placement is limited to controls.
  <div modelValue={text} />,
];
