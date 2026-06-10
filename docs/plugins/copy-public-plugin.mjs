// zfb plugin module: copy-public.
//
// TRANSITION NOTE (#932): This plugin is kept while the docs pin uses
// @takazudo/zfb@0.1.0-next.35, which pre-dates the `copyPublicWithBase`
// knob added in this change. The new knob (`copyPublicWithBase: false` in
// docs/zfb.config.ts) handles the flat copy natively in next.37+, but the
// released binary doesn't know it yet so silently ignores it. Remove this
// plugin when the docs dependency pin is bumped to the version that ships
// `copyPublicWithBase` (i.e. when docs/package.json references next.37+).
//
// Original rationale (still accurate):
// postBuild — recursively copies `<projectRoot>/public/` directly into
//             `<outDir>/` (FLAT, matching zfb's own dist/ convention —
//             zfb emits dist/index.html, dist/assets/..., NOT
//             dist/<base>/index.html).
//
//             Deploy target is Cloudflare Pages under base
//             `/pj/zudo-front-builder` (settings.base). The deploy
//             workflow (.github/workflows/*-deploy.yml) relocates the
//             flat build into the base path itself:
//               cp -a docs/dist/. deploy-root/pj/zudo-front-builder/
//             so `public/img/logo.svg` → `dist/img/logo.svg` lands at
//             `deploy-root/pj/zudo-front-builder/img/logo.svg`, served at
//             `/pj/zudo-front-builder/img/logo.svg` — i.e. the flat copy
//             this plugin produces is the load-bearing artifact. zfb also
//             emits a base-prefixed tree under `dist/pj/...`, but that
//             would deploy to `.../pj/zudo-front-builder/pj/...` (double
//             base) and is unused dead weight at deploy time — which is
//             why this plugin copies flat and ignores `base`.
//
// Missing or empty `public/` is treated as a no-op (no error).
//
// `options` carries `{ publicDir }` from the matching entry in
// `zfb.config.ts`. The `base` option is intentionally unused — see
// rationale above.

import { cp } from "node:fs/promises";
import { resolve } from "node:path";

export default {
  name: "copy-public",

  async postBuild(ctx) {
    const { publicDir: publicDirOption } = ctx.options;
    const publicDir = resolve(ctx.projectRoot, publicDirOption ?? "public");
    const dest = ctx.outDir;

    ctx.logger.info(`copying ${publicDir} → ${dest}`);

    await cp(publicDir, dest, {
      recursive: true,
      force: true,
      errorOnExist: false,
    }).catch((err) => {
      if (err.code === "ENOENT") {
        // publicDir does not exist or is empty — treat as no-op.
        ctx.logger.info("public/ not found — skipping copy");
        return;
      }
      throw err;
    });
  },
};
