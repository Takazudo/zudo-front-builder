import { signal } from "../src/zudo-react/index.js";

const text = signal("text");
const checked = signal(false);
const chosen = signal<string | null>(null);
export const forms = [
  <input modelValue={text} />,
  <textarea modelValue={text} />,
  <select modelValue={text}>
    <option value="text">Text</option>
  </select>,
  <input type="checkbox" modelChecked={checked} />,
  <input type="radio" name="group" value="text" modelValue={chosen} />,
  // @ts-expect-error File inputs cannot have models.
  <input type="file" modelValue={text} />,
  // @ts-expect-error Checkbox models are boolean.
  <input type="checkbox" modelChecked={text} />,
  // @ts-expect-error Model placement is limited to controls.
  <div modelValue={text} />,
];
