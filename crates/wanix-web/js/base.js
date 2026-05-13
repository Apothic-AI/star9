export class WanixElement extends HTMLElement {
    constructor() {
        super();
        this._system = null;
        this._awakeStarted = false;
        this._taskpath = "#task";
    }

    connectedCallback() {
        if (this.hasAttribute("task-ns")) {
            this._taskpath = this.getAttribute("task-ns") || "#task";
        }

        if (this.tagName === "WANIX-SYSTEM") {
            return;
        }

        this._system = resolveSystemElement(this);
        if (!this._system) {
            throw new Error("Component element must be a child of a wanix-system element");
        }

        void this._waitForSystem();
    }

    async _waitForSystem() {
        if (this._awakeStarted) {
            return;
        }
        this._awakeStarted = true;
        await this._system.ready;
        await this._awake();
    }

    _awake() {
        throw new Error("Not implemented");
    }
}

function resolveSystemElement(element) {
    if (element.hasAttribute("for")) {
        const target = document.getElementById(element.getAttribute("for"));
        if (target && target.tagName !== "WANIX-SYSTEM") {
            throw new Error("Component element target must be a wanix-system element");
        }
        return target;
    }

    return element.closest("wanix-system");
}
