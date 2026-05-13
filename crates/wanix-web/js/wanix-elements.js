import { BindElement } from "./bind.js";
import { SystemElement } from "./system.js";
import { TaskElement } from "./task.js";

defineElement("wanix-system", SystemElement);
defineElement("wanix-bind", BindElement);
defineElement("wanix-task", TaskElement);

export { BindElement, SystemElement, TaskElement };

function defineElement(name, ctor) {
    if (typeof window === "undefined" || !window.customElements) {
        return;
    }
    if (!window.customElements.get(name)) {
        window.customElements.define(name, ctor);
    }
}
