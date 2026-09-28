"use client";

import { useState } from "preact/hooks";

export default function ClientOnly() {
  const [open, setOpen] = useState(false);
  return (
    <section>
      <button type="button" onClick={() => setOpen(!open)}>
        Toggle client-only class
      </button>
      {open && <div class="bg-client-only">The client-only branch is open.</div>}
    </section>
  );
}
