import { WanixElement } from "./base.js";
import { attachWanixImportResponder } from "./p9-port.js";
import { BrowserJsWasmWorkerHost } from "./js-wasm-worker-host.js";
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

    spawnWorker(workerId, parentTaskId = "1") {
        return this.system.spawnWorker(workerId, parentTaskId || "");
    }

    startWorker(worker, execution) {
        return this.system.startWorker(worker, normalizeExecutionSpec(execution));
    }

    openWorkerPort(worker, port) {
        return this.system.openWorkerPort(worker, normalizePortDescriptor(port));
    }

    handoffWorkerPort(worker, targetTaskId, port) {
        return this.system.handoffWorkerPort(worker, String(targetTaskId), normalizePortDescriptor(port));
    }

    recordWorkerExit(taskId, workerId, sequence, code) {
        return this.system.recordWorkerExit(String(taskId), workerId || "", BigInt(sequence || 0), Number(code || 0));
    }

    recordWorkerStdout(taskId, workerId, sequence, data, eof = false) {
        return this.system.recordWorkerStdout(
            String(taskId),
            workerId || "",
            BigInt(sequence || 0),
            toUint8Array(data),
            Boolean(eof),
        );
    }

    async startBrowserWorker(source, options = {}) {
        const workerId = String(options.workerId || options.worker_id || `browser-worker-${Date.now()}`);
        const worker = this.spawnWorker(workerId, options.parentTaskId || options.parent_task_id || "1");
        const execution = normalizeExecutionSpec({
            kind: options.kind || "js_wasm",
            module: options.module || "../../../tests/fixtures/js-wasm-execution-runner.mjs",
            args: options.args || [],
            env: options.env || [],
            cwd: options.cwd || ".",
            stdio: options.stdio,
            fds: options.fds || [],
            ports: options.ports || [],
        });
        this.startWorker(worker, execution);

        const host = BrowserJsWasmWorkerHost.spawn(source, {
            ...options,
            system: this,
            taskId: worker.task_id,
            workerId: worker.worker_id,
            execution,
        });
        const controller = new BrowserWorkerTaskController(this, worker, host);
        controller.start();
        await host.start();
        return controller;
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

class BrowserWorkerTaskController {
    constructor(system, worker, host) {
        this.system = system;
        this.worker = worker;
        this.host = host;
        this.messages = [];
        this.sequence = 0;
        this.exitCode = null;
        this._cleanup = [];
    }

    get taskId() {
        return this.worker.task_id;
    }

    get workerId() {
        return this.worker.worker_id;
    }

    start() {
        this._cleanup.push(
            this.host.onTaskMessage((message) => {
                this._handleTaskMessage(message);
            }),
        );
        return this;
    }

    close() {
        for (const cleanup of this._cleanup.splice(0)) {
            cleanup();
        }
        this.host.close();
    }

    _handleTaskMessage(message) {
        const payload = decodeTaskPayload(message.payload);
        this.messages.push(payload);
        const sequence = ++this.sequence;
        if (isExecutionExitPayload(payload)) {
            this.exitCode = Number(payload.exit_code || 0);
            this.system.recordWorkerExit(this.taskId, this.workerId, sequence, this.exitCode);
            return;
        }
        if (typeof payload === "string") {
            this.system.recordWorkerStdout(this.taskId, this.workerId, sequence, message.payload, false);
        }
    }
}

function normalizeExecutionSpec(value = {}) {
    const env = Array.isArray(value.env)
        ? value.env.map((entry) =>
            typeof entry === "string"
                ? splitEnvEntry(entry)
                : { name: String(entry.name || ""), value: String(entry.value || "") },
        )
        : [];
    return {
        kind: normalizeExecutionKind(value.kind || "js_wasm"),
        module: String(value.module || ""),
        args: Array.from(value.args || [], String),
        env,
        cwd: value.cwd == null ? null : String(value.cwd),
        stdio: value.stdio || {
            stdin: { kind: "Inherit" },
            stdout: { kind: "Inherit" },
            stderr: { kind: "Inherit" },
        },
        fds: Array.from(value.fds || []),
    };
}

function normalizeExecutionKind(kind) {
    const value = String(kind).trim().toLowerCase().replace(/-/g, "_");
    if (value === "wasi") {
        return "wasi";
    }
    if (value === "js" || value === "js_wasm" || value === "gojs" || value === "go_js") {
        return "js_wasm";
    }
    return value;
}

function normalizePortDescriptor(port) {
    if (typeof port === "string") {
        return { port_id: port, name: port };
    }
    return {
        port_id: String(port.port_id || port.portId || ""),
        name: String(port.name || port.port_id || port.portId || ""),
    };
}

function splitEnvEntry(entry) {
    const text = String(entry);
    const index = text.indexOf("=");
    return index < 0
        ? { name: text, value: "" }
        : { name: text.slice(0, index), value: text.slice(index + 1) };
}

function decodeTaskPayload(payload) {
    const bytes = toUint8Array(payload);
    try {
        const text = new TextDecoder().decode(bytes);
        return JSON.parse(text);
    } catch {
        try {
            return new TextDecoder().decode(bytes);
        } catch {
            return bytes;
        }
    }
}

function isExecutionExitPayload(payload) {
    return payload && typeof payload === "object" && payload.type === "wanix-js-wasm-execution-exit";
}

function toUint8Array(value) {
    if (value instanceof Uint8Array) {
        return value;
    }
    if (value instanceof ArrayBuffer) {
        return new Uint8Array(value);
    }
    if (ArrayBuffer.isView(value)) {
        return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    if (Array.isArray(value)) {
        return Uint8Array.from(value);
    }
    return new TextEncoder().encode(String(value));
}
