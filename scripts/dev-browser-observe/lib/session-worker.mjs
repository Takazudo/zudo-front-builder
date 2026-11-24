import { execute, EXIT } from "./run.mjs";

const args = JSON.parse(process.argv[2]);
const controller = new AbortController();
const abort = (reason) => controller.abort(new Error(reason));
const onMessage = (message) => {
  if (message?.type === "abort") abort(message.reason);
};
const onDisconnect = () => abort("session supervisor disconnected");
process.on("message", onMessage);
process.on("disconnect", onDisconnect);
try {
  if (Number(process.hrtime.bigint() / 1_000_000n) >= args.workDeadline)
    abort("session wall cleanup reserve reached during startup");
  controller.signal.throwIfAborted();
  process.exitCode = await execute({
    ...args,
    signal: controller.signal,
    onFinalizing: () => {
      if (process.connected) process.send({ type: "finalizing" }, () => {});
    },
  });
} catch (error) {
  process.stderr.write(`${error?.stack ?? error}\n`);
  process.exitCode = error?.code === "PROFILE_INVALID" ? EXIT.usage : EXIT.controllerError;
} finally {
  process.off("message", onMessage);
  process.off("disconnect", onDisconnect);
  if (process.connected) {
    process.send({ type: "finished" }, () => {
      if (process.connected) process.disconnect();
    });
  }
}
