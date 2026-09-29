"use client";
import { sharedCount } from "./shared-signal";

export function SharedReader() {
  return <output id="shared-reader">Read: {sharedCount}</output>;
}
