"use client";
import { signal } from "@takazudo/zfb/zudo-react";
export function Model() {
  const name = signal("Ada");
  return (
    <label>
      Name <input modelValue={name} />
      <output>{name}</output>
    </label>
  );
}
