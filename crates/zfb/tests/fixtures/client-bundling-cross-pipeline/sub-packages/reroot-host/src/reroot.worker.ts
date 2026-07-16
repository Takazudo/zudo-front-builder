// Project-local module worker for the claimed sub-package host. Its own graph
// reaches a SIBLING workspace `?raw` (spelled relative), while the worker
// SOURCE stays under the host project so the #1500 flat-naming contract holds.
import workerPanel from "../../reroot-shared/worker-panel.frag?raw";

self.postMessage(["ZFB_REROOT_WORKER_ENTRY", workerPanel]);
