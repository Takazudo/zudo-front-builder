/** Compatibility exports must remain the helper's exact functions/classes. */
import { describe, expect, it } from "vite-plus/test";
import {
  slugify as helperSlugify,
  SlugAllocator as HelperSlugAllocator,
} from "@takazudo/zfb-slugify";
import { slugify as subpathSlugify, SlugAllocator as SubpathSlugAllocator } from "../slugify.js";
import { slugify as rootSlugify, SlugAllocator as RootSlugAllocator } from "../index.js";

describe("slugify compatibility routes", () => {
  it("re-exports the helper from the SDK root and subpath", () => {
    expect(rootSlugify).toBe(helperSlugify);
    expect(subpathSlugify).toBe(helperSlugify);
    expect(RootSlugAllocator).toBe(HelperSlugAllocator);
    expect(SubpathSlugAllocator).toBe(HelperSlugAllocator);
    expect(rootSlugify("Hello, World!")).toBe("hello-world");
    const allocator = new SubpathSlugAllocator();
    expect(allocator.allocate(2, rootSlugify("Hello, World!"))).toBe("hello-world");
    expect(allocator.allocate(2, rootSlugify("Hello, World!"))).toBe("hello-world-1");
  });
});
