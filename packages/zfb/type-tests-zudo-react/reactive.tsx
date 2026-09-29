import { computed, signal, type ReadonlySignal, type Signal } from "@takazudo/zfb/zudo-react";

const count: Signal<number> = signal(1);
const text: ReadonlySignal<string> = computed(() => String(count.value));
const view = <div title={text}>{text}</div>;
count.value = 2;
// @ts-expect-error Computed values are readonly.
text.value = "no";
// @ts-expect-error Numeric signals do not satisfy string attributes.
const invalid = <div title={count} />;
void [view, invalid];
