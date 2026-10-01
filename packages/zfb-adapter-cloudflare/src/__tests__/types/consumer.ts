import {
  getCloudflareContext,
  runWithCloudflareContext,
  type CloudflareContext,
  type CloudflareExecutionContext,
} from "@takazudo/zfb-adapter-cloudflare";

interface Env {
  API_TOKEN: string;
}

interface WorkersCacheContext {
  purge(): Promise<void>;
}

type ExtendedCtx = CloudflareExecutionContext & {
  cache?: WorkersCacheContext;
};

const defaultContext = getCloudflareContext();
const defaultEnv: unknown = defaultContext.env;
const defaultRequest: Request = defaultContext.request;
void defaultEnv;
void defaultRequest;

// @ts-expect-error The default env type remains unknown until a caller narrows it.
const defaultEnvAsString: string = defaultContext.env;
void defaultEnvAsString;

// The default context deliberately exposes only the minimal execution shape.
// @ts-expect-error The default context does not promise a cache binding.
defaultContext.ctx.cache;

const envOnlyContext = getCloudflareContext<Env>();
const apiToken: string = envOnlyContext.env.API_TOKEN;
const envOnlyCtx: CloudflareExecutionContext = envOnlyContext.ctx;
const envOnlyRequest: Request = envOnlyContext.request;
void apiToken;
void envOnlyCtx;
void envOnlyRequest;

// @ts-expect-error Env-only calls do not add undeclared bindings.
envOnlyContext.env.MISSING_BINDING;

const extendedContext = getCloudflareContext<Env, ExtendedCtx>();
const extendedToken: string = extendedContext.env.API_TOKEN;
const extendedRequest: Request = extendedContext.request;
void extendedToken;
void extendedRequest;

async function purgeIfCacheIsAvailable(
  context: CloudflareContext<Env, ExtendedCtx>,
): Promise<void> {
  if (context.ctx.cache) {
    await context.ctx.cache.purge();
  }

  // @ts-expect-error Optional cache access must be guarded before use.
  await context.ctx.cache.purge();
}

void purgeIfCacheIsAvailable;

// A richer caller context can be inferred while keeping the existing return
// type generic first in runWithCloudflareContext.
const inferredResult: string = runWithCloudflareContext(
  {
    env: { API_TOKEN: "token" },
    ctx: {
      waitUntil: () => undefined,
      passThroughOnException: () => undefined,
      cache: { purge: async () => undefined },
    },
    request: new Request("https://example.test/"),
    marker: "extra context data",
  },
  () => "complete",
);
void inferredResult;

const minimalCtx: CloudflareExecutionContext = {
  waitUntil: () => undefined,
  passThroughOnException: () => undefined,
};
const baseContext: CloudflareContext<Env> = {
  env: { API_TOKEN: "token" },
  ctx: minimalCtx,
  request: new Request("https://example.test/"),
};
const explicitResult: number = runWithCloudflareContext<number>(baseContext, () => 42);
void explicitResult;

// @ts-expect-error Extended Ctx types must retain both required execution methods.
getCloudflareContext<Env, { cache?: WorkersCacheContext }>();

runWithCloudflareContext(
  {
    env: { API_TOKEN: "token" },
    // @ts-expect-error Contexts must include both required execution methods.
    ctx: { cache: { purge: async () => undefined } },
    request: new Request("https://example.test/"),
  },
  () => undefined,
);
