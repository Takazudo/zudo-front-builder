// Project-local module worker for the sibling host. Its graph reaches a SIBLING
// workspace `?raw` (spelled relative) while the worker SOURCE stays under the
// host project.
import workerPanel from "../../shared/worker-panel.frag?raw";

self.postMessage(["ZFB_SIBLING_WORKER_ENTRY", workerPanel]);
