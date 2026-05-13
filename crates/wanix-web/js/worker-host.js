import { connectRuntimePort } from "./worker-runtime.js";

export class BrowserWorkerHost {
    constructor(options = {}) {
        if (!options || typeof options !== "object") {
            throw new TypeError("expected BrowserWorkerHost options to be an object");
        }

        this._target = options.target ?? options.worker ?? null;
        this._createTarget = resolveTargetFactory(options);
        this._ownsTarget =
            options.ownsTarget ?? (this._target == null && this._createTarget != null);
        this._terminateOnClose = options.terminateOnClose !== false;
        this._connectOptions = buildConnectOptions(options);

        this.target = null;
        this.endpoint = null;
        this.port = null;
        this.descriptor = null;
        this.bootstrap = null;
        this.element = null;
        this.facade = null;

        this._started = false;
        this._closed = false;
        this._targetListenersActive = false;

        this._listeners = new Set();
        this._requestListeners = new Set();
        this._responseListeners = new Set();
        this._taskListeners = new Set();
        this._targetMessageListeners = new Set();
        this._errorListeners = new Set();

        this._handleTargetMessage = this._handleTargetMessage.bind(this);
        this._handleTargetError = this._handleTargetError.bind(this);
        this._handleTargetMessageError = this._handleTargetMessageError.bind(this);

        if (typeof options.onmessage === "function") {
            this.onMessage(options.onmessage);
        }
        if (typeof options.onrequest === "function") {
            this.onRequest(options.onrequest);
        }
        if (typeof options.onresponse === "function") {
            this.onResponse(options.onresponse);
        }
        if (typeof options.ontaskmessage === "function") {
            this.onTaskMessage(options.ontaskmessage);
        }
        if (typeof options.ontargetmessage === "function") {
            this.onTargetMessage(options.ontargetmessage);
        }
        if (typeof options.onerror === "function") {
            this.onError(options.onerror);
        }
    }

    static attach(target, options = {}) {
        return new BrowserWorkerHost({ ...options, target, ownsTarget: false });
    }

    static spawn(source, options = {}) {
        return new BrowserWorkerHost(normalizeSpawnOptions(source, options));
    }

    get started() {
        return this._started;
    }

    get closed() {
        return this._closed;
    }

    onMessage(listener) {
        return addListener(this._listeners, listener, "worker host listener");
    }

    onRequest(listener) {
        return addListener(this._requestListeners, listener, "worker host request listener");
    }

    onResponse(listener) {
        return addListener(this._responseListeners, listener, "worker host response listener");
    }

    onTaskMessage(listener) {
        return addListener(this._taskListeners, listener, "worker host task listener");
    }

    onTargetMessage(listener) {
        return addListener(this._targetMessageListeners, listener, "worker target message listener");
    }

    onError(listener) {
        return addListener(this._errorListeners, listener, "worker host error listener");
    }

    async start() {
        if (this._closed) {
            throw new Error("BrowserWorkerHost is closed");
        }

        if (!this.endpoint) {
            const target = await this._resolveTarget();
            this.target = target;
            this._attachTargetListeners();
            const connection = await connectRuntimePort(target, this._connectOptions);
            this.endpoint = connection.endpoint;
            this.port = connection.port;
            this.descriptor = connection.descriptor;
            this.bootstrap = connection.bootstrap;
            this.element = connection.element;
            this.facade = connection.facade;
            this._bindEndpoint(connection.endpoint);
        }

        this.endpoint.start();
        this._attachTargetListeners();
        this._started = true;
        return this;
    }

    stop() {
        if (!this.endpoint) {
            return this;
        }

        this.endpoint.stop();
        this._detachTargetListeners();
        this._started = false;
        return this;
    }

    close(options = {}) {
        if (this._closed) {
            return this;
        }

        const terminate = options.terminate ?? (this._ownsTarget && this._terminateOnClose);

        this.stop();
        if (this.port && typeof this.port.close === "function") {
            this.port.close();
        }
        if (terminate && this.target && typeof this.target.terminate === "function") {
            this.target.terminate();
        }

        this.port = null;
        this.endpoint = null;
        this.bootstrap = null;
        this.descriptor = null;
        this.element = null;
        this.facade = null;
        this._started = false;
        this._closed = true;
        return this;
    }

    dispose(options = {}) {
        return this.close(options);
    }

    send(kind, payload) {
        return requireEndpoint(this).send(kind, payload);
    }

    sendRequest(payload) {
        return requireEndpoint(this).sendRequest(payload);
    }

    sendResponse(payload) {
        return requireEndpoint(this).sendResponse(payload);
    }

    sendTaskMessage(payload) {
        return requireEndpoint(this).sendTaskMessage(payload);
    }

    async _resolveTarget() {
        if (this._target) {
            return this._target;
        }
        if (!this._createTarget) {
            throw new Error("BrowserWorkerHost requires a target, createTarget, or worker URL");
        }
        const target = await this._createTarget();
        if (!target || typeof target !== "object") {
            throw new TypeError("worker target factory must return a Worker-like object");
        }
        this._target = target;
        return target;
    }

    _bindEndpoint(endpoint) {
        endpoint.onMessage((message) => {
            emitListeners(this._listeners, message);
        });
        endpoint.onRequest((message) => {
            emitListeners(this._requestListeners, message);
        });
        endpoint.onResponse((message) => {
            emitListeners(this._responseListeners, message);
        });
        endpoint.onTaskMessage((message) => {
            emitListeners(this._taskListeners, message);
        });
        endpoint.onError((error) => {
            emitListeners(this._errorListeners, error);
        });
    }

    _attachTargetListeners() {
        if (!this.target || this._targetListenersActive) {
            return;
        }
        this.target.addEventListener("message", this._handleTargetMessage);
        this.target.addEventListener("error", this._handleTargetError);
        this.target.addEventListener("messageerror", this._handleTargetMessageError);
        this._targetListenersActive = true;
    }

    _detachTargetListeners() {
        if (!this.target || !this._targetListenersActive) {
            return;
        }
        this.target.removeEventListener("message", this._handleTargetMessage);
        this.target.removeEventListener("error", this._handleTargetError);
        this.target.removeEventListener("messageerror", this._handleTargetMessageError);
        this._targetListenersActive = false;
    }

    _handleTargetMessage(event) {
        emitListeners(this._targetMessageListeners, event);
    }

    _handleTargetError(event) {
        emitListeners(this._errorListeners, event?.error ?? event);
    }

    _handleTargetMessageError(event) {
        emitListeners(this._errorListeners, event?.error ?? event);
    }
}

export async function acceptBrowserWorkerHost(target, options = {}) {
    return BrowserWorkerHost.attach(target, options).start();
}

export async function spawnBrowserWorkerHost(source, options = {}) {
    return BrowserWorkerHost.spawn(source, options).start();
}

function normalizeSpawnOptions(source, options) {
    if (isWorkerLike(source)) {
        return { ...options, target: source, ownsTarget: false };
    }
    if (typeof source === "function") {
        return { ...options, createTarget: source, ownsTarget: true };
    }
    if (source != null) {
        return { ...options, workerUrl: source, ownsTarget: true };
    }
    return { ...options };
}

function resolveTargetFactory(options) {
    if (typeof options.createTarget === "function") {
        return options.createTarget;
    }
    if (typeof options.createWorker === "function") {
        return options.createWorker;
    }

    const workerUrl = options.workerUrl ?? options.url ?? options.src ?? null;
    if (workerUrl == null) {
        return null;
    }

    return () => spawnWorkerTarget(workerUrl, options.workerOptions);
}

function spawnWorkerTarget(workerUrl, workerOptions = {}) {
    if (typeof Worker !== "function") {
        throw new Error(
            "Browser Worker constructor is unavailable; provide a Worker-like target or createTarget",
        );
    }

    const resolvedUrl = resolveWorkerUrl(workerUrl, workerOptions.baseUrl);
    const { baseUrl: _unusedBaseUrl, ...spawnOptions } = workerOptions;
    return new Worker(resolvedUrl, { type: "module", ...spawnOptions });
}

function resolveWorkerUrl(workerUrl, baseUrl) {
    if (workerUrl instanceof URL) {
        return workerUrl.href;
    }

    const base = baseUrl ?? inferBaseUrl();
    return new URL(String(workerUrl), base).href;
}

function inferBaseUrl() {
    if (typeof document !== "undefined" && document.baseURI) {
        return document.baseURI;
    }
    return import.meta.url;
}

function buildConnectOptions(options) {
    const connectOptions = {
        descriptor: options.descriptor,
        endpoint: options.endpoint,
        system: options.system,
        type: options.type,
        message: options.message,
        portKey: options.portKey,
        descriptorKey: options.descriptorKey,
    };

    const taskId = options.taskId ?? options.task_id;
    if (taskId != null) {
        connectOptions.taskId = taskId;
    }

    const workerId = options.workerId ?? options.worker_id;
    if (workerId != null) {
        connectOptions.workerId = workerId;
    }

    return connectOptions;
}

function requireEndpoint(host) {
    if (!host.endpoint) {
        throw new Error("BrowserWorkerHost is not started");
    }
    return host.endpoint;
}

function isWorkerLike(value) {
    return (
        !!value &&
        typeof value === "object" &&
        typeof value.postMessage === "function" &&
        typeof value.addEventListener === "function" &&
        typeof value.removeEventListener === "function"
    );
}

function addListener(bucket, listener, label) {
    if (typeof listener !== "function") {
        throw new TypeError(`expected ${label} to be a function`);
    }
    bucket.add(listener);
    return () => {
        bucket.delete(listener);
    };
}

function emitListeners(listeners, value) {
    for (const listener of listeners) {
        listener(value);
    }
}
