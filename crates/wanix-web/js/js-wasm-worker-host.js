import { DEFAULT_RUNTIME_PORT_MESSAGE_TYPE } from "./worker-runtime.js";
import { BrowserWorkerHost } from "./worker-host.js";

export const DEFAULT_JS_WASM_EXECUTION_KIND = "js_wasm";
export const DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE = "wanix-js-wasm-execution";

export class BrowserJsWasmWorkerHost {
    constructor(options = {}) {
        if (!options || typeof options !== "object") {
            throw new TypeError("expected BrowserJsWasmWorkerHost options to be an object");
        }

        this.host = resolveWorkerHost(options);
        this.execution = normalizeExecutionOptions(options);
        this._bootstrapType = options.bootstrapType || DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE;
        this._bootstrapMessage = cloneRecord(options.bootstrapMessage || null);
        this._bootstrapTransfer = normalizeTransferList(
            options.bootstrapTransfer ?? options.transfer ?? [],
        );

        this.bootstrap = null;
        this._bootstrapPosted = false;
    }

    static attach(target, options = {}) {
        return new BrowserJsWasmWorkerHost({
            ...options,
            workerHost: BrowserWorkerHost.attach(target, options),
        });
    }

    static spawn(source, options = {}) {
        return new BrowserJsWasmWorkerHost({
            ...options,
            workerHost: BrowserWorkerHost.spawn(source, options),
        });
    }

    get target() {
        return this.host.target;
    }

    get port() {
        return this.host.port;
    }

    get descriptor() {
        return this.host.descriptor;
    }

    get endpoint() {
        return this.host.endpoint;
    }

    get element() {
        return this.host.element;
    }

    get facade() {
        return this.host.facade;
    }

    get runtimeBootstrap() {
        return this.host.bootstrap;
    }

    get started() {
        return this.host.started;
    }

    get closed() {
        return this.host.closed;
    }

    onMessage(listener) {
        return this.host.onMessage(listener);
    }

    onRequest(listener) {
        return this.host.onRequest(listener);
    }

    onResponse(listener) {
        return this.host.onResponse(listener);
    }

    onTaskMessage(listener) {
        return this.host.onTaskMessage(listener);
    }

    onError(listener) {
        return this.host.onError(listener);
    }

    send(kind, payload) {
        return this.host.send(kind, payload);
    }

    sendRequest(payload) {
        return this.host.sendRequest(payload);
    }

    sendResponse(payload) {
        return this.host.sendResponse(payload);
    }

    sendTaskMessage(payload) {
        return this.host.sendTaskMessage(payload);
    }

    async start() {
        await this.host.start();
        if (!this._bootstrapPosted) {
            this.bootstrap = createJsWasmExecutionBootstrap({
                type: this._bootstrapType,
                message: this._bootstrapMessage,
                execution: this.execution,
                task_id: this.runtimeBootstrap?.task_id,
                worker_id: this.runtimeBootstrap?.worker_id,
                runtime: buildRuntimeDescriptor(this.runtimeBootstrap, this.descriptor),
            });
            postJsWasmExecutionBootstrap(this.target, this.bootstrap, this._bootstrapTransfer);
            this._bootstrapPosted = true;
        }
        return this;
    }

    stop() {
        this.host.stop();
        return this;
    }

    close(options = {}) {
        this.host.close(options);
        this.bootstrap = null;
        return this;
    }

    dispose(options = {}) {
        return this.close(options);
    }
}

export async function attachBrowserJsWasmWorkerHost(target, options = {}) {
    return BrowserJsWasmWorkerHost.attach(target, options).start();
}

export async function spawnBrowserJsWasmWorkerHost(source, options = {}) {
    return BrowserJsWasmWorkerHost.spawn(source, options).start();
}

export function createJsWasmExecutionBootstrap(options = {}) {
    if (!options || typeof options !== "object") {
        throw new TypeError("expected JS/WASM execution bootstrap options to be an object");
    }

    const execution = normalizeBootstrapExecution(options.execution || options);
    const taskId = normalizeOptionalString(options.task_id ?? options.taskId);
    const workerId = normalizeOptionalString(options.worker_id ?? options.workerId);
    const runtime = cloneRecord(options.runtime || null);

    const bootstrap = {
        ...cloneRecord(options.message || null),
        type:
            normalizeOptionalString(options.type) ||
            DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE,
        kind: execution.kind,
        module: execution.module,
        args: execution.args,
        env: execution.env,
        cwd: execution.cwd,
        stdio: execution.stdio,
        fds: execution.fds,
        ports: execution.ports,
    };

    if (taskId != null) {
        bootstrap.task_id = taskId;
    }
    if (workerId != null) {
        bootstrap.worker_id = workerId;
    }
    if (runtime && Object.keys(runtime).length > 0) {
        bootstrap.runtime = runtime;
    }

    return bootstrap;
}

export function postJsWasmExecutionBootstrap(target, bootstrap, transfer = []) {
    if (!target || typeof target.postMessage !== "function") {
        throw new TypeError("expected a Worker-like target with postMessage");
    }
    target.postMessage(bootstrap, normalizeTransferList(transfer));
    return bootstrap;
}

function resolveWorkerHost(options) {
    const host = options.workerHost ?? options.host ?? null;
    if (host != null) {
        validateWorkerHost(host);
        return host;
    }
    return new BrowserWorkerHost(options);
}

function validateWorkerHost(host) {
    const requiredMethods = [
        "start",
        "stop",
        "close",
        "send",
        "sendRequest",
        "sendResponse",
        "sendTaskMessage",
        "onMessage",
        "onRequest",
        "onResponse",
        "onTaskMessage",
        "onError",
    ];

    for (const method of requiredMethods) {
        if (typeof host[method] !== "function") {
            throw new TypeError(
                `expected worker host to expose method ${JSON.stringify(method)}`,
            );
        }
    }
}

function normalizeExecutionOptions(options) {
    return normalizeBootstrapExecution({
        kind: options.kind ?? options.executionKind ?? DEFAULT_JS_WASM_EXECUTION_KIND,
        module: options.module,
        args: options.args,
        env: options.env,
        cwd: options.cwd,
        stdio: options.stdio,
        fds: options.fds,
        ports: options.ports,
    });
}

function normalizeBootstrapExecution(execution) {
    if (!execution || typeof execution !== "object") {
        throw new TypeError("expected JS/WASM execution options to be an object");
    }

    const module = normalizeRequiredString(execution.module, "module");
    return {
        kind: normalizeRequiredString(
            execution.kind ?? DEFAULT_JS_WASM_EXECUTION_KIND,
            "kind",
        ),
        module,
        args: normalizeStringArray(execution.args, "args"),
        env: normalizeEnvironment(execution.env),
        cwd: normalizeOptionalString(execution.cwd),
        stdio: cloneRecord(execution.stdio || null),
        fds: normalizeArrayOfRecords(execution.fds, "fds"),
        ports: normalizeArrayOfRecords(execution.ports, "ports"),
    };
}

function buildRuntimeDescriptor(runtimeBootstrap, descriptor) {
    const runtime = {
        type:
            normalizeOptionalString(runtimeBootstrap?.type) ||
            DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
        descriptor: cloneRecord(descriptor || null),
    };
    const systemId = normalizeOptionalString(runtimeBootstrap?.system_id);
    if (systemId != null) {
        runtime.system_id = systemId;
    }
    return runtime;
}

function normalizeEnvironment(value) {
    if (value == null) {
        return [];
    }

    if (Array.isArray(value)) {
        return value.map((entry, index) => normalizeEnvironmentEntry(entry, index));
    }

    if (typeof value === "object") {
        return Object.entries(value).map(([name, entryValue]) => ({
            name: String(name),
            value: String(entryValue),
        }));
    }

    throw new TypeError("expected env to be an array or object");
}

function normalizeEnvironmentEntry(entry, index) {
    if (!entry || typeof entry !== "object") {
        throw new TypeError(`expected env[${index}] to be an object`);
    }

    return {
        name: normalizeRequiredString(entry.name, `env[${index}].name`),
        value: normalizeRequiredString(entry.value, `env[${index}].value`),
    };
}

function normalizeArrayOfRecords(value, label) {
    if (value == null) {
        return [];
    }
    if (!Array.isArray(value)) {
        throw new TypeError(`expected ${label} to be an array`);
    }
    return value.map((entry, index) => {
        if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
            throw new TypeError(`expected ${label}[${index}] to be an object`);
        }
        return cloneRecord(entry);
    });
}

function normalizeStringArray(value, label) {
    if (value == null) {
        return [];
    }
    if (!Array.isArray(value)) {
        throw new TypeError(`expected ${label} to be an array`);
    }
    return value.map((entry, index) => {
        if (entry == null) {
            throw new TypeError(`expected ${label}[${index}] to be a string`);
        }
        return String(entry);
    });
}

function normalizeRequiredString(value, label) {
    const normalized = normalizeOptionalString(value);
    if (normalized == null) {
        throw new TypeError(`expected ${label} to be a non-empty string`);
    }
    return normalized;
}

function normalizeOptionalString(value) {
    if (value == null) {
        return null;
    }
    const normalized = String(value);
    return normalized.length > 0 ? normalized : null;
}

function normalizeTransferList(value) {
    if (value == null) {
        return [];
    }
    if (!Array.isArray(value)) {
        throw new TypeError("expected transfer to be an array");
    }
    return value.slice();
}

function cloneRecord(value) {
    if (value == null) {
        return null;
    }
    if (Array.isArray(value) || typeof value !== "object") {
        throw new TypeError("expected descriptor/message values to be objects");
    }
    return structuredCloneCompat(value);
}

function structuredCloneCompat(value) {
    if (typeof structuredClone === "function") {
        return structuredClone(value);
    }
    return JSON.parse(JSON.stringify(value));
}
