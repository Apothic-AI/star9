import { WanixElement } from "./base.js";

const DEFAULT_FACADE_URL = new URL("../../../target/wanix-web-pkg/wanix_web.js", import.meta.url).href;
const facadeModules = new Map();

export class SystemElement extends WanixElement {
    constructor() {
        super();
        this._facade = null;
        this._initStarted = false;
        this.isReady = false;
        this.ready = new Promise((resolve, reject) => {
            this._resolveReady = resolve;
            this._rejectReady = reject;
        });
    }

    connectedCallback() {
        super.connectedCallback();
        if (this._initStarted) {
            return;
        }
        this._initStarted = true;
        void this._initialize();
    }

    get facadeUrl() {
        const value = this.getAttribute("pkg");
        if (!value) {
            return DEFAULT_FACADE_URL;
        }
        return new URL(value, document.baseURI).href;
    }

    get system() {
        if (!this._facade) {
            throw new Error("wanix-system is not ready");
        }
        return this._facade;
    }

    readText(path) {
        return this.system.readText(path);
    }

    readFile(path) {
        return this.system.readFile(path);
    }

    writeText(path, value) {
        return this.system.writeText(path, value);
    }

    writeFile(path, value) {
        return this.system.writeFile(path, value);
    }

    readDir(path) {
        return this.system.readDir(path);
    }

    setupNamespace(taskId, bindings) {
        return this.system.setupNamespace(taskId, bindings);
    }

    setupNamespaceJson(taskId, json) {
        return this.system.setupNamespaceJson(taskId, json);
    }

    bindRamFs(dst) {
        return this.system.bindRamFs(dst);
    }

    mountSelf9p(dst) {
        return this.system.mountSelf9p(dst);
    }

    startTask(kind, command) {
        return this.system.startTask(kind, command);
    }

    startWasi(command) {
        return this.system.startWasi(command);
    }

    startGoJs(command) {
        return this.system.startGoJs(command);
    }

    async applyBindings(bindings, taskId = "1", taskPath = this._taskpath) {
        for (const element of bindings) {
            if (typeof element?.plan !== "function") {
                continue;
            }

            const plan = await element.plan();
            const { binding } = plan;

            if (binding.kind === "ns" && binding.src === "#ramfs" && taskId === "1") {
                this.system.bindRamFs(binding.dst);
                continue;
            }

            if (binding.storage || binding.kind === "ns") {
                this.system.setupNamespace(taskId, [binding]);
                continue;
            }

            if (binding.kind === "file") {
                const src = binding.src || generatedBindingSource("inline", taskId, binding.dst);
                this.system.registerFileBytes(src, plan.fileBytes);
                this.system.setupNamespace(taskId, [{ ...binding, src }]);
                continue;
            }

            if (binding.kind === "archive") {
                const src = binding.src || generatedBindingSource("archive", taskId, binding.dst);
                this.system.registerArchiveBytes(src, plan.archiveBytes);
                this.system.setupNamespace(taskId, [{ ...binding, src }]);
                continue;
            }

            if (binding.kind === "import") {
                throw new Error(
                    "import bindings require a browser 9P MessagePort transport adapter",
                );
            }
        }
    }

    async _initialize() {
        try {
            const { WanixSystem } = await loadFacadeModule(this.facadeUrl);
            this._facade = new WanixSystem();
            await this.applyBindings(this.querySelectorAll(":scope > wanix-bind"), "1");
            this.isReady = true;
            this._resolveReady(this);
            this.dispatchEvent(
                new CustomEvent("ready", {
                    bubbles: true,
                    detail: { system: this },
                }),
            );
        } catch (error) {
            this._rejectReady(error);
            this.dispatchEvent(
                new CustomEvent("error", {
                    bubbles: true,
                    detail: { error },
                }),
            );
        }
    }
}

async function loadFacadeModule(url) {
    let pending = facadeModules.get(url);
    if (!pending) {
        pending = import(url).then(async (module) => {
            await module.default();
            return module;
        });
        facadeModules.set(url, pending);
    }
    return pending;
}

function generatedBindingSource(kind, taskId, path) {
    return `wanix:${kind}:${taskId}:${path}`;
}
