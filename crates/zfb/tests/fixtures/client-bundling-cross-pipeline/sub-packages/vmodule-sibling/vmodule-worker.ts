// Nested worker registered by vmodule-host.ts (issue #1701). Its own SOURCE
// stays under this sibling package rather than under reroot-host, so the
// #1500 flat-naming contract's workspace-scoped `worker--ws-` branch
// (`module_worker_filename_scoped`) is the one exercised here, not the
// project-local branch reroot.worker.ts already covers.
self.postMessage(["ZFB_VMODULE_WORKER_ENTRY"]);
