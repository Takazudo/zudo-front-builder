import { signal } from "@takazudo/zfb/zudo-react";

let value: ReturnType<typeof signal<string>>;

export default function SyntheticCompositionProbe() {
  value = signal("start");
  return (
    <section>
      <input id="composition-input" modelValue={value} />
      <output id="composition-value">{value}</output>
    </section>
  );
}

export function run() {
  return { value };
}
