import { Star9Element } from "./base.js";
import { attachStar9ImportResponder } from "./p9-port.js";
import { BrowserJsWasmWorkerHost } from "./js-wasm-worker-host.js";
import {
    AsyncMountTable,
    createBrowserStorageAdapter,
    createP9MountFromPort,
    dirEntriesToNames,
} from "./mounts.js";
import { createStorageP9FramePort } from "./storage-p9.js";
import { requestStar9ImportPort } from "./worker-runtime.js";

const DEFAULT_FACADE_URL = new URL("../../../target/star9-web-pkg/star9_web.js", import.meta.url).href;
const facadeModules = new Map();

export function callStar9Logger(logger, operation, args = []) {
    if (typeof logger !== "function") {
        return false;
    }
    try {
        logger(String(operation), ...Array.from(args || []));
        return true;
    } catch {
        return false;
    }
}

export class SystemElement extends Star9Element {
    constructor() {
        super();
        this._facade = null;
        this._initStarted = false;
        this._mounts = new AsyncMountTable();
        this._importResponder = null;
        this.logger = () => null;
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
            throw new Error("star9-system is not ready");
        }
        return this._facade;
    }

    readText(path) {
        this._log("readText", path);
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.readText(mount.path);
        }
        return this.system.readText(path);
    }

    readFile(path) {
        this._log("readFile", path);
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.readFile(mount.path);
        }
        return this.system.readFile(path);
    }

    writeText(path, value) {
        this._log("writeText", path);
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.writeText(mount.path, value);
        }
        return this.system.writeText(path, value);
    }

    writeFile(path, value) {
        this._log("writeFile", path);
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.writeFile(mount.path, value);
        }
        return this.system.writeFile(path, value);
    }

    writeExistingText(path, value) {
        this._log("writeExistingText", path);
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.writeText(mount.path, value);
        }
        return this.system.writeExistingText(path, value);
    }

    writeExistingFile(path, value) {
        this._log("writeExistingFile", path);
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.writeFile(mount.path, value);
        }
        return this.system.writeExistingFile(path, value);
    }

    readDir(path) {
        this._log("readDir", path);
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.readDir(mount.path).then(dirEntriesToNames);
        }
        return this.system.readDir(path);
    }

    stat(path) {
        this._log("stat", path);
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.stat(mount.path);
        }
        return this.system.stat(path);
    }

    mkdir(path) {
        this._log("mkdir", path);
        const mount = this._mounts.resolve(path);
        if (mount) {
            return mount.adapter.mkdir(mount.path);
        }
        return this.system.mkdir(path);
    }

    remove(path) {
        this._log("remove", path);
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
        this._log("bindRamFs", dst);
        return this.system.bindRamFs(dst);
    }

    mountSelf9p(dst) {
        this._log("mountSelf9p", dst);
        return this.system.mountSelf9p(dst);
    }

    async mountStorage(dst, descriptor, options = {}) {
        this._log("mountStorage", dst);
        const adapter = await createBrowserStorageAdapter(descriptor, {
            globals: globalThis,
            ...options,
        });
        this.mountAdapter(dst, adapter, {
            kind: "storage",
            source: descriptor?.backend || null,
        });
        return adapter;
    }

    async createStorageExport(descriptor, options = {}) {
        this._log("createStorageExport", descriptor?.backend || "");
        const storageAdapter = options.adapter || await createBrowserStorageAdapter(descriptor, {
            globals: globalThis,
            ...options,
        });
        const exported = createStorageP9FramePort(storageAdapter, options.p9 || options);
        const mountAdapter = await createP9MountFromPort(exported.port, options.mount || options);
        const closeMount = typeof mountAdapter.close === "function" ? mountAdapter.close.bind(mountAdapter) : null;
        mountAdapter.storageAdapter = storageAdapter;
        mountAdapter.storageExportServer = exported.server;
        mountAdapter.close = () => {
            closeMount?.();
            exported.server.close();
            if (typeof storageAdapter.close === "function") {
                storageAdapter.close();
            }
        };
        return mountAdapter;
    }

    async mountStorageExport(dst, descriptor, options = {}) {
        this._log("mountStorageExport", dst);
        const adapter = await this.createStorageExport(descriptor, options);
        this.mountAdapter(dst, adapter, {
            kind: options.kind || "storage-9p",
            source: descriptor?.backend || options.source || null,
        });
        return adapter;
    }

    async mountTaskStorage(taskId, dst, descriptor, options = {}) {
        const id = requireNonEmptyString(taskId, "task id");
        return this.mountStorageExport(normalizeTaskNamespaceMountPath(id, dst), descriptor, {
            ...options,
            kind: options.kind || "task-storage-9p",
        });
    }

    async mountStarFs(dst, descriptor = {}, options = {}) {
        return this.mountStorageExport(dst, { ...descriptor, backend: "starfs" }, {
            ...options,
            kind: options.kind || "starfs",
        });
    }

    async mountTaskStarFs(taskId, dst, descriptor = {}, options = {}) {
        return this.mountTaskStorage(taskId, dst, { ...descriptor, backend: "starfs" }, {
            ...options,
            kind: options.kind || "task-starfs",
        });
    }

    mountAdapter(dst, adapter, options = {}) {
        this._log("mountAdapter", dst);
        return this._mounts.mount(dst, adapter, options);
    }

    async mountImportPort(dst, port, options = {}) {
        this._log("mountImportPort", dst);
        const adapter = await createP9MountFromPort(port, options);
        this.mountAdapter(dst, adapter, {
            kind: "9p",
            source: options.source || null,
        });
        return adapter;
    }

    async createExportMount(port, options = {}) {
        this._log("createExportMount", options.source || "");
        const ready = options.waitForReady === false ? null : await waitForExportReady(port, options);
        const adapter = await createP9MountFromPort(port, options);
        adapter.readySignal = ready;
        return adapter;
    }

    mountTaskExportAdapter(taskId, adapter, options = {}) {
        const id = requireNonEmptyString(taskId, "task id");
        return this.mountAdapter(`#task/${id}/export`, adapter, {
            kind: "worker-export",
            source: options.source || null,
        });
    }

    async mountWorkerExport(taskId, port, options = {}) {
        const adapter = await this.createExportMount(port, {
            source: options.source || "worker-export",
            ...options,
        });
        this.mountTaskExportAdapter(taskId, adapter, options);
        return adapter;
    }

    mountVmGuestAdapter(vmId, adapter, options = {}) {
        const id = requireNonEmptyString(vmId, "VM id");
        return this.mountAdapter(`#vm/${id}/guest`, adapter, {
            kind: "vm-guest",
            source: options.source || null,
        });
    }

    async mountVmGuest(vmId, port, options = {}) {
        const adapter = await this.createExportMount(port, {
            source: options.source || "vm-guest",
            ...options,
        });
        this.mountVmGuestAdapter(vmId, adapter, options);
        return adapter;
    }

    async mountImport(dst, src, options = {}) {
        this._log("mountImport", dst, src);
        const port = await requestStar9ImportPort(src, options);
        return this.mountImportPort(dst, port, { ...options, source: src });
    }

    async mountBrowserService(dst, source, options = {}) {
        this._log("mountBrowserService", dst, source);
        const descriptor = parseBrowserServiceSource(source, options);
        if (descriptor.family === "webtransport") {
            throw new Error("browser WebTransport service provider is not configured");
        }
        return this.mountWebSocket9p(dst, descriptor.url, {
            ...options,
            source,
            family: descriptor.family,
        });
    }

    async mountNetworkService(dst, source, options = {}) {
        return this.mountBrowserService(dst, source, options);
    }

    async mountWebSocket9p(dst, url, options = {}) {
        this._log("mountWebSocket9p", dst, url);
        const socketTarget = await createWebSocketP9Target(url, options);
        const adapter = await createP9MountFromPort(socketTarget, options.mount || options);
        const closeMount = typeof adapter.close === "function" ? adapter.close.bind(adapter) : null;
        adapter.close = () => {
            closeMount?.();
            socketTarget.close?.();
        };
        this.mountAdapter(dst, adapter, {
            kind: "browser-network-9p",
            source: options.source || url,
        });
        return adapter;
    }

    unmount(dst) {
        this._log("unmount", dst);
        return this._mounts.unmount(dst);
    }

    startTask(kind, command) {
        this._log("startTask", kind, command);
        return this.system.startTask(kind, command);
    }

    startWasi(command) {
        this._log("startWasi", command);
        return this.system.startWasi(command);
    }

    startGoJs(command) {
        this._log("startGoJs", command);
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
        this._log("startBrowserWorker", source);
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
            const { Star9System } = await loadFacadeModule(this.facadeUrl);
            this._facade = new Star9System();
            await this.applyBindings(this.querySelectorAll(":scope > star9-bind"), "1");
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

    _log(operation, ...args) {
        callStar9Logger(this.logger, operation, args);
    }

    async _maybeExportImportPort() {
        if (typeof window === "undefined" || !this.id || !this.hasAttribute("allow-origins")) {
            return;
        }
        this._importResponder = await attachStar9ImportResponder(window, this, {
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
    return `star9:${kind}:${taskId}:${path}`;
}

class BrowserWorkerTaskController {
    constructor(system, worker, host) {
        this.system = system;
        this.worker = worker;
        this.host = host;
        this.messages = [];
        this.exportMounts = [];
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
        if (typeof this.host.onTargetMessage === "function") {
            this._cleanup.push(
                this.host.onTargetMessage((event) => {
                    this._handleWorkerMessage(event);
                }),
            );
        }
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

    _handleWorkerMessage(event) {
        const payload = event?.data ?? event;
        if (!payload || typeof payload !== "object" || payload.export == null) {
            return;
        }
        void this._installExport(payload).catch((error) => {
            this.messages.push({
                type: "star9-worker-export-error",
                error: errorMessage(error),
            });
        });
    }

    async _installExport(payload) {
        const adapter = await this.system.createExportMount(payload.export, {
            source: "worker-export",
            waitForReady: true,
            readyTimeoutMs: payload.readyTimeoutMs,
        });
        this.system.mountTaskExportAdapter(this.taskId, adapter, {
            source: "worker-export",
        });
        const record = {
            type: "star9-worker-export-ready",
            task_id: this.taskId,
            vm: payload.vm == null ? null : String(payload.vm),
        };
        if (payload.vm != null) {
            this.system.mountVmGuestAdapter(String(payload.vm), adapter, {
                source: "worker-export",
            });
        }
        this.exportMounts.push(record);
        this.messages.push(record);
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
    if (value === "native" || value === "process" || value === "pty") {
        return "native";
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
    return payload && typeof payload === "object" && payload.type === "star9-js-wasm-execution-exit";
}

function waitForExportReady(port, options = {}) {
    const messagePort = requireMessagePortLike(port);
    const timeoutMs = Number(options.readyTimeoutMs ?? 1000);
    return new Promise((resolve, reject) => {
        let timeout = null;
        const cleanup = () => {
            if (timeout !== null) {
                clearTimeout(timeout);
            }
            messagePort.removeEventListener("message", onMessage);
        };
        const onMessage = (event) => {
            cleanup();
            resolve(event?.data);
        };
        messagePort.addEventListener("message", onMessage);
        if (timeoutMs >= 0) {
            timeout = setTimeout(() => {
                cleanup();
                reject(new Error(`timed out waiting for worker export ready signal after ${timeoutMs}ms`));
            }, timeoutMs);
        }
        messagePort.start();
    });
}

function requireMessagePortLike(port) {
    if (
        !port ||
        typeof port.postMessage !== "function" ||
        typeof port.addEventListener !== "function" ||
        typeof port.removeEventListener !== "function" ||
        typeof port.start !== "function"
    ) {
        throw new TypeError("expected a MessagePort-like worker export port");
    }
    return port;
}

function requireNonEmptyString(value, label) {
    const normalized = String(value ?? "").trim();
    if (!normalized) {
        throw new TypeError(`${label} must not be empty`);
    }
    return normalized;
}

function normalizeTaskNamespaceMountPath(taskId, dst) {
    const clean = String(dst ?? "").trim();
    if (!clean || clean === ".") {
        throw new TypeError("task storage destination must not be empty");
    }
    if (clean.startsWith("/") || clean.includes("\\") || clean.split("/").includes("..")) {
        throw new TypeError(`task storage destination must be a relative Star9 path: ${JSON.stringify(dst)}`);
    }
    return `#task/${taskId}/ns/${clean.split("/").filter(Boolean).join("/")}`;
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

function parseBrowserServiceSource(source, options = {}) {
    const explicitFamily = options.family ? String(options.family) : "";
    const text = String(source || "");
    const bang = text.indexOf("!");
    const family = explicitFamily || (bang >= 0 ? text.slice(0, bang) : "");
    const rest = bang >= 0 ? text.slice(bang + 1) : text;
    if (family === "ws" || family === "wss") {
        return { family, url: browserFamilyUrl(family, options.url || rest) };
    }
    if (family === "webtransport") {
        return { family, url: browserFamilyUrl("https", options.url || rest) };
    }
    throw new Error(`unknown browser service family: ${source}`);
}

function browserFamilyUrl(scheme, address) {
    const value = String(address || "");
    if (/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(value)) {
        return value;
    }
    const [host, ...pathParts] = value.split("!");
    if (!host) {
        throw new Error("browser service address is missing a host");
    }
    const path = pathParts.join("/");
    if (!path) {
        return `${scheme}://${host}`;
    }
    return `${scheme}://${host}${path.startsWith("/") ? "" : "/"}${path}`;
}

async function createWebSocketP9Target(url, options = {}) {
    const WebSocketCtor = options.WebSocket || globalThis.WebSocket;
    if (typeof WebSocketCtor !== "function") {
        throw new Error("browser WebSocket service provider is not available");
    }
    const socket = new WebSocketCtor(url);
    socket.binaryType = "arraybuffer";
    const target = new WebSocketMessageTarget(socket);
    await target.ready(Number(options.openTimeoutMs ?? 3000));
    return target;
}

class WebSocketMessageTarget {
    constructor(socket) {
        this.socket = socket;
        this._listeners = new Map();
    }

    addEventListener(type, listener) {
        if (type !== "message") {
            this.socket.addEventListener?.(type, listener);
            return;
        }
        const wrapped = (event) => listener({ data: event.data, originalEvent: event });
        this._listeners.set(listener, wrapped);
        this.socket.addEventListener("message", wrapped);
    }

    removeEventListener(type, listener) {
        if (type !== "message") {
            this.socket.removeEventListener?.(type, listener);
            return;
        }
        const wrapped = this._listeners.get(listener);
        if (wrapped) {
            this.socket.removeEventListener("message", wrapped);
            this._listeners.delete(listener);
        }
    }

    postMessage(payload) {
        this.socket.send(toUint8Array(payload));
    }

    start() {
        return this;
    }

    close() {
        this.socket.close?.();
    }

    ready(timeoutMs) {
        if (this.socket.readyState === 1) {
            return Promise.resolve();
        }
        return new Promise((resolve, reject) => {
            let timeout = null;
            const cleanup = () => {
                if (timeout !== null) {
                    clearTimeout(timeout);
                }
                this.socket.removeEventListener?.("open", onOpen);
                this.socket.removeEventListener?.("error", onError);
                this.socket.removeEventListener?.("close", onClose);
            };
            const onOpen = () => {
                cleanup();
                resolve();
            };
            const onError = (event) => {
                cleanup();
                reject(new Error(event?.message || "browser WebSocket service failed to open"));
            };
            const onClose = () => {
                cleanup();
                reject(new Error("browser WebSocket service closed before opening"));
            };
            this.socket.addEventListener?.("open", onOpen);
            this.socket.addEventListener?.("error", onError);
            this.socket.addEventListener?.("close", onClose);
            if (timeoutMs >= 0) {
                timeout = setTimeout(() => {
                    cleanup();
                    reject(new Error(`timed out opening browser WebSocket service after ${timeoutMs}ms`));
                }, timeoutMs);
            }
        });
    }
}

function errorMessage(error) {
    return error instanceof Error ? error.message : String(error);
}
