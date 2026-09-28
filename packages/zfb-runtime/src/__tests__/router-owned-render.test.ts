import { expect, it } from "vitest";
import { jsx } from "@takazudo/zfb/zudo-react/jsx-runtime";

import { createPageRouter } from "../router.js";

it("renders a branded owned description through the server entry", async () => {
  const router = createPageRouter({
    pages: [
      {
        route: "/",
        module: async () => ({
          default: () => jsx("main", { children: jsx("strong", { children: "owned" }) }),
        }),
      },
    ],
    contentSnapshot: { collections: {} },
  });
  const response = await router(new Request("https://example.test/"));
  expect(response.status).toBe(200);
  expect(await response.text()).toContain("<main><strong>owned</strong></main>");
});
