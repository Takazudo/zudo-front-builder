# @takazudo/zfb-slugify

Standalone, zero-dependency heading-anchor slug utilities used by zfb. The
package exports only `slugify` and `SlugAllocator`.

```sh
npm install @takazudo/zfb-slugify
```

```ts
import { SlugAllocator, slugify } from "@takazudo/zfb-slugify";

const allocator = new SlugAllocator("hierarchical");
const slug = allocator.allocate(2, slugify("Hello World")); // "hello-world"
```

The output matches zfb's Rust implementation through the shared fixture at
`crates/zfb-content/tests/fixtures/slugify-parity.json`. The existing
`@takazudo/zfb/slugify` export remains available for compatibility.

`SlugAllocator` accepts `"flat"` (the default) or `"hierarchical"`; call
`reset()` between documents to clear deduplication and ancestor state.
