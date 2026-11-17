// Compile-only consumers: run by the package's normal typecheck script.
import { createPageRouter, stablePaths } from "../src/server.js";
import { stablePaths as directStablePaths } from "@takazudo/zfb-runtime/stable-paths";
// @ts-expect-error Snapshot brand inspection is not part of the public entry.
import { isStablePaths } from "@takazudo/zfb-runtime/stable-paths";
import type {
  CreatePageRouterOptions,
  PageDefinition,
  PageModule,
  StablePathsEntry,
} from "../src/server.js";

void isStablePaths;

// Existing bare public annotations must keep their mutable paths() return type,
// including consumers that obtain a module through a definition/options object.
declare const legacyModule: PageModule;
const moduleEntries: unknown[] = await legacyModule.paths!();
moduleEntries.push({ params: { slug: "new" } });
(await legacyModule.paths!()).push({ params: { slug: "direct" } });

declare const legacyDefinition: PageDefinition;
const definitionEntries: unknown[] = await (await legacyDefinition.module()).paths!();
definitionEntries.push({ params: { slug: "new" } });
(await (await legacyDefinition.module()).paths!()).push({ params: { slug: "direct" } });

declare const legacyOptions: CreatePageRouterOptions;
const optionsEntries: unknown[] = await (await legacyOptions.pages[0]!.module()).paths!();
optionsEntries.push({ params: { slug: "new" } });
(await (await legacyOptions.pages[0]!.module()).paths!()).push({ params: { slug: "direct" } });
createPageRouter(legacyOptions);

const directSnapshot = directStablePaths([{ params: { slug: "direct" } }]);
createPageRouter({
  pages: [
    {
      route: "/direct/:slug",
      module: async () => ({ paths: () => directSnapshot, default: () => "ok" }),
    },
  ],
  contentSnapshot: { collections: {} },
});
// @ts-expect-error The lightweight public entry preserves readonly membership.
directSnapshot.push({ params: { slug: "other" } });

interface InterfaceParams {
  slug: string;
}
interface InterfaceProps {
  title: string;
}
const interfaceParams: InterfaceParams = { slug: "typed" };
const interfaceProps: InterfaceProps = { title: "hello" };
const interfaceSnapshot = directStablePaths([{ params: interfaceParams, props: interfaceProps }]);
const interfaceTitle: string = interfaceSnapshot[0]!.props!.title;
interfaceSnapshot[0]!.props!.title = "still mutable";
class OpaqueProps {
  title = "opaque";
}
const opaqueProps = new OpaqueProps();
const opaqueSnapshot = directStablePaths([{ params: interfaceParams, props: opaqueProps }]);
const retainedProps: OpaqueProps = opaqueSnapshot[0]!.props!;
interface OptionalParams {
  slug?: string | readonly (string | number)[];
  page?: number;
}
const heterogeneousEntries: readonly { params: OptionalParams; props?: InterfaceProps }[] = [
  { params: {} },
  { params: { slug: [] } },
  { params: { slug: ["a", 42], page: 2 }, props: interfaceProps },
];
directStablePaths(heterogeneousEntries);
directStablePaths([]);
void interfaceTitle;
void retainedProps;

const props = { title: "hello", params: { mutable: true } };
const snapshot = stablePaths([{ params: { slug: ["a", 42] as const }, props }] as const);
const synchronous: PageModule<readonly StablePathsEntry[]> = {
  paths: () => snapshot,
  default: () => "ok",
};
const asynchronous: PageModule<typeof snapshot> = {
  paths: async () => snapshot,
  default: () => "ok",
};
const definition: PageDefinition<typeof snapshot> = {
  route: "/:slug",
  module: async () => asynchronous,
};
const options: CreatePageRouterOptions<typeof snapshot> = {
  pages: [definition],
  contentSnapshot: { collections: {} },
};
createPageRouter(options);
createPageRouter({
  pages: [
    { route: "/sync/:slug", module: async () => synchronous },
    { route: "/async/:slug", module: async () => asynchronous },
    {
      route: "/inferred/:slug",
      module: async () => ({ paths: () => snapshot, default: () => "ok" }),
    },
    legacyDefinition,
  ],
  contentSnapshot: { collections: {} },
});

snapshot[0]!.props!.title = "still mutable";
// @ts-expect-error owned array is readonly
snapshot.push({ params: { slug: "b" }, props });
// @ts-expect-error owned params are readonly
snapshot[0]!.params.slug = "b";
// @ts-expect-error parameter arrays are readonly
(snapshot[0]!.params.slug as readonly string[]).push("b");
// @ts-expect-error explicitly stable module returns a readonly array
(await synchronous.paths!()).push({ params: { slug: "b" } });
// @ts-expect-error stable typing propagates through definitions
(await (await definition.module()).paths!()).push({ params: { slug: "b" } });
// @ts-expect-error stable typing propagates through options
(await (await options.pages[0]!.module()).paths!()).push({ params: { slug: "b" } });
// @ts-expect-error a readonly snapshot cannot satisfy the mutable default
const mutablePromise: PageModule = asynchronous;
// @ts-expect-error coercion objects are outside the opt-in contract
stablePaths([{ params: { slug: { toString: () => "a" } } }]);
void mutablePromise;
