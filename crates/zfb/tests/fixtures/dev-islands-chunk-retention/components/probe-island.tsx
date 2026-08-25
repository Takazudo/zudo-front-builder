"use client";

export function ProbeIsland() {
  return (
    <button
      id="probe-island"
      type="button"
      onClick={async (event) => {
        const button = event.currentTarget;
        const { lazyPart } = await import("./lazy-part");
        button.textContent = lazyPart;
      }}
    >
      probe island
    </button>
  );
}
