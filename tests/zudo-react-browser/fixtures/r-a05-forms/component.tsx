import { computed, signal } from "@takazudo/zfb/zudo-react";

let state: {
  text: ReturnType<typeof signal<string>>;
  area: ReturnType<typeof signal<string>>;
  checked: ReturnType<typeof signal<boolean>>;
  choice: ReturnType<typeof signal<string>>;
  radio: ReturnType<typeof signal<string | null>>;
  summary: ReturnType<typeof computed<string>>;
};

export default function FormsProbe() {
  const text = signal("text default");
  const area = signal("area default");
  const checked = signal(false);
  const choice = signal("one");
  const radio = signal<string | null>("left");
  const summary = computed(
    () => `${text.value}|${area.value}|${checked.value}|${choice.value}|${radio.value}`,
  );
  state = { text, area, checked, choice, radio, summary };

  return (
    <form id="forms">
      <input id="uncontrolled" defaultValue="uncontrolled default" />
      <input id="model-text" modelValue={text} />
      <textarea id="model-area" modelValue={area} />
      <input id="model-checked" type="checkbox" modelChecked={checked} />
      <select id="model-choice" modelValue={choice}>
        <option value="one">One</option>
        <option value="two">Two</option>
      </select>
      <label>
        <input type="radio" name="side" value="left" modelValue={radio} /> Left
      </label>
      <label>
        <input type="radio" name="side" value="right" modelValue={radio} /> Right
      </label>
      <output id="model-summary">{summary}</output>
    </form>
  );
}

export function UnsupportedMultiple() {
  const model = signal("one");
  return (
    <select id="unsupported-multiple" multiple modelValue={model}>
      <option value="one">One</option>
      <option value="two">Two</option>
    </select>
  );
}

export function run() {
  return state;
}
