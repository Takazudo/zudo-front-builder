import { computed, flush, signal } from "@takazudo/zfb/zudo-react";

interface CounterProps {
  initial: number;
}

export default function Counter({ initial }: CounterProps) {
  const count = signal(initial);
  return (
    <button
      id="counter"
      on:click={() => {
        count.value += 1;
      }}
    >
      Count: {count}
    </button>
  );
}

export async function run() {
  const source = signal(2);
  const doubled = computed(() => source.value * 2);
  source.value = 3;
  await flush();
  return { settled: doubled.value };
}
