// Interrupt a wait whose underlying operation is read-only or disposed during
// finalization. The caller must separately prevent any subsequent workload edit.
export async function interruptible(work, signal) {
  if (!signal) return work;
  let abort;
  try {
    return await Promise.race([
      work,
      new Promise((_, reject) => {
        abort = () => reject(signal.reason);
        if (signal.aborted) abort();
        else signal.addEventListener("abort", abort, { once: true });
      }),
    ]);
  } finally {
    signal.removeEventListener("abort", abort);
  }
}
