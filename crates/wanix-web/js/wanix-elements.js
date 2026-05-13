import { BindElement } from "./bind.js";
import { SystemElement } from "./system.js";
import { TaskElement } from "./task.js";

defineElement("wanix-system", SystemElement);
defineElement("wanix-bind", BindElement);
defineElement("wanix-task", TaskElement);

export { BindElement, SystemElement, TaskElement };
export * from "./js-wasm-execution-worker.js";
export * from "./js-wasm-worker-host.js";
export * from "./p9-port.js";
export * from "./mounts.js";
export * from "./worker-host.js";
export * from "./worker-runtime.js";
export * from "./storage-file-system.js";
export * from "./storage-web.js";
export * from "./storage-js-value.js";

function defineElement(name, ctor) {
    if (typeof window === "undefined" || !window.customElements) {
        return;
    }
    if (!window.customElements.get(name)) {
        window.customElements.define(name, ctor);
    }
}
