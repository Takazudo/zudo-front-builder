import { extra } from "./worker-dep";
self.onmessage = (event) => self.postMessage(event.data + extra);
