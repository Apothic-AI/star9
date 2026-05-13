import { WanixElement } from "./base.js";
import { attachWanixImportResponder } from "./p9-port.js";
import {
    AsyncMountTable,
    createBrowserStorageAdapter,
    createP9MountFromPort,
    dirEntriesToNames,
} from "./mounts.js";
import { requestWanixImportPort } from "./worker-runtime.js";

const DEFAULT_FACADE_URL = new URL("../../../target/wanix-web-pkg/wanix_web.js", import.meta.url).href;
const facadeModules = new Map();

export class SystemElement extends WanixElement {
    constructor() {
        super();
        this._facade = null;
        this._initStarted = false;
        this._mounts = new AsyncMountTable();
        this._importResponder = null;
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
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.readText(mount.path);
        }
        return this.system.readText(path);
    }

    readFile(path) {
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.readFile(mount.path);
        }
        return this.system.readFile(path);
    }

    writeText(path, value) {
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.writeText(mount.path, value);
        }
        return this.system.writeText(path, value);
    }

    writeFile(path, value) {
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.writeFile(mount.path, value);
        }
        return this.system.writeFile(path, value);
    }

    readDir(path) {
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.readDir(mount.path).then(dirEntriesToNames);
        }
        return this.system.readDir(path);
    }

    stat(path) {
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.stat(mount.path);
        }
        return this.system.stat(path);
    }

    mkdir(path) {
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.mkdir(mount.path);
        }
        return this.system.mkdir(path);
    }

    remove(path) {
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.remove(mount.path);
        }
        return this.system.remove(path);
    }

    async setupNamespace(taskId, bindings) {
        const nativeBindings = [];
        for (const binding of bindings || []) {
            if (binding?.storage) {
                await this.mountStorage(binding.dst, binding.storage);
            } else {
                nativeBindings.push(binding);
            }
        }
        if (nativeBindings.length > 0) {
            this.system.setupNamespace(taskId, nativeBindings);
        }
    }

    async setupNamespaceJson(taskId, json) {
        return this.setupNamespace(taskId, JSON.parse(json));
    }

    bindRamFs(dst) {
        return this.system.bindRamFs(dst);
    }

    mountSelf9p(dst) {
        return this.system.mountSelf9p(dst);
    }

    async mountStorage(dst, descriptor, options = {}) {
        const adapter = await createBrowserStorageAdapter(descriptor, {
            globals: globalThis,
            ...options,
        });
        this._mounts.mount(dst, adapter, {
            kind: "storage",
            source: descriptor?.backend || null,
        });
        return adapter;
    }

    async mountImportPort(dst, port, options = {}) {
        const adapter = await createP9MountFromPort(port, options);
        this._mounts.mount(dst, adapter, {
            kind: "9p",
            source: options.source || null,
        });
        return adapter;
    }

    async mountImport(dst, src, options = {}) {
        const port = await requestWanixImportPort(src, options);
        return this.mountImportPort(dst, port, { ...options, source: src });
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
                await this.setupNamespace(taskId, [binding]);
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
                const port = await plan.importPlaceholder.requestPort();
                await this.mountImportPort(binding.dst, port, { source: binding.src });
                continue;
            }
        }
    }

    async _initialize() {
        try {
            const { WanixSystem } = await loadFacadeModule(this.facadeUrl);
            this._facade = new WanixSystem();
            await this.applyBindings(this.querySelectorAll(":scope > wanix-bind"), "1");
            await this._maybeExportImportPort();
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

    disconnectedCallback() {
        this._importResponder?.close();
        this._importResponder = null;
        this._mounts.close();
    }

    async _maybeExportImportPort() {
        if (typeof window === "undefined" || !this.id || !this.hasAttribute("allow-origins")) {
            return;
        }
        this._importResponder = await attachWanixImportResponder(window, this, {
            allowOrigins: this.getAttribute("allow-origins") || "",
            systemId: this.id,
        });
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
