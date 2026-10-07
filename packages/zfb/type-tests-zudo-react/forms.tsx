import { signal, computed, type ReadonlySignal } from "@takazudo/zfb/zudo-react";

const text = signal("text");
const checked = signal(false);
const chosen = signal<string | null>(null);
const readonlyText: ReadonlySignal<string> = text;
const readonlyChecked: ReadonlySignal<boolean> = checked;
const readonlyChosen: ReadonlySignal<string | null> = chosen;
const shown: ReadonlySignal<string> = computed(() => text.value);
const flag = signal(true);
const dynamicType: string = Math.random() > 0.5 ? "text" : "number";
const textLikeType: "text" | "search" = Math.random() > 0.5 ? "text" : "search";
export const forms = [
  <input type="text" value="a" />,
  <input readonly value={computed(() => text.value)} />,
  <input disabled value={shown} />,
  <input type="search" readonly value={shown} />,
  <textarea readonly value={shown} />,
  <textarea disabled value={shown} />,
  <input type="checkbox" value="v" />,
  <input type="radio" value="v" />,
  <input type="color" value="#fff" />,
  <select value="a">
    <option value="a">A</option>
  </select>,
  <button value="b" />,
  <textarea value="static" />,
  <input type="checkbox" checked={true} />,
  <input type="checkbox" checked={null} />,
  <option selected={checked}>Selected reactively</option>,
  <my-widget value="a" checked={null} />,
  <input modelValue={text} />,
  <input defaultValue="plain" />,
  <input type="text" defaultValue="plain" />,
  <input type="search" defaultValue="plain" />,
  <input type="email" defaultValue="plain" />,
  <input type="url" defaultValue="plain" />,
  <input type="tel" defaultValue="plain" />,
  <input type="password" defaultValue="plain" />,
  <input type={textLikeType} defaultValue="plain" />,
  <input type="checkbox" defaultChecked />,
  <input type="radio" defaultChecked />,
  <input type="radio" modelValue={undefined} defaultChecked />,
  <input type="radio" name="group" value="plain" defaultChecked />,
  <textarea defaultValue="plain" />,
  <select defaultValue="plain">
    <option value="plain">Plain</option>
  </select>,
  <input
    type="number"
    value="2"
    on:input={(event) => {
      event.currentTarget.value = "2";
    }}
  />,
  <input type="color" value="#ff0000" />,
  // @ts-expect-error Reactive text values require a static readonly or disabled attribute.
  <input value={shown} />,
  // @ts-expect-error A reactive readonly attribute does not qualify.
  <input readonly={flag} value={shown} />,
  // @ts-expect-error A false readonly attribute does not qualify.
  <input readonly={false} value={shown} />,
  // @ts-expect-error Number inputs do not qualify for display-only bindings.
  <input type="number" readonly value={shown} />,
  // @ts-expect-error Dynamic input types do not qualify for display-only bindings.
  <input type={dynamicType} readonly value={shown} />,
  // @ts-expect-error A display-only value cannot coexist with a model.
  <input readonly value={shown} modelValue={text} />,
  // @ts-expect-error A display-only value cannot coexist with a default.
  <input readonly value={shown} defaultValue="x" />,
  // @ts-expect-error Reactive textarea values require a static readonly or disabled attribute.
  <textarea value={shown} />,
  // @ts-expect-error A display-only textarea value cannot coexist with a model.
  <textarea disabled value={shown} modelValue={text} />,
  // @ts-expect-error Omitted type is text and cannot use defaultChecked.
  <input defaultChecked />,
  // @ts-expect-error Text inputs cannot use defaultChecked.
  <input type="text" defaultChecked />,
  // @ts-expect-error Checkboxes cannot use defaultValue.
  <input type="checkbox" defaultValue="on" />,
  // @ts-expect-error Radios cannot use defaultValue.
  <input type="radio" name="group" value="plain" defaultValue="plain" />,
  // @ts-expect-error Modelled radios require a name.
  <input type="radio" value="plain" modelValue={chosen} />,
  // @ts-expect-error Modelled radios require a value.
  <input type="radio" name="group" modelValue={chosen} />,
  // @ts-expect-error Unknown or dynamic input types cannot claim text defaults.
  <input type={dynamicType} defaultValue="plain" />,
  // @ts-expect-error Unknown or dynamic input types cannot claim checked defaults.
  <input type={dynamicType} defaultChecked />,
  // @ts-expect-error Color inputs cannot use defaultValue.
  <input type="color" defaultValue="#ff0000" />,
  // @ts-expect-error Number inputs cannot use defaultValue.
  <input type="number" defaultValue="2" />,
  // @ts-expect-error Range inputs cannot use defaultValue.
  <input type="range" defaultValue="2" />,
  // @ts-expect-error Date inputs cannot use defaultValue.
  <input type="date" defaultValue="2026-10-05" />,
  // @ts-expect-error Time inputs cannot use defaultValue.
  <input type="time" defaultValue="12:00" />,
  // @ts-expect-error Unsupported literal inputs cannot use defaultChecked.
  <input type="number" defaultChecked />,
  // @ts-expect-error Selects cannot use defaultChecked.
  <select defaultChecked>
    <option value="plain">Plain</option>
  </select>,
  // @ts-expect-error Textareas cannot use defaultChecked.
  <textarea defaultChecked />,
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
