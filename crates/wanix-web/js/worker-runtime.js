const REQUEST_TAG = 1;
const RESPONSE_TAG = 2;
const TASK_MESSAGE_TAG = 3;

const TAG_BY_KIND = Object.freeze({
    request: REQUEST_TAG,
    response: RESPONSE_TAG,
    task: TASK_MESSAGE_TAG,
});

const KIND_BY_TAG = Object.freeze({
    [REQUEST_TAG]: "request",
    [RESPONSE_TAG]: "response",
    [TASK_MESSAGE_TAG]: "task",
});

export const WORKER_RUNTIME_MESSAGE_TAGS = TAG_BY_KIND;
export const DEFAULT_RUNTIME_PORT_MESSAGE_TYPE = "wanix-worker-runtime";

export function encodeWorkerRuntimeEnvelope(kind, payload) {
    const tag = tagForKind(kind);
    const body = cloneBytes(payload, `worker runtime ${kind} payload`);
    const message = new Uint8Array(1 + body.byteLength);
    message[0] = tag;
    message.set(body, 1);
    return message;
}

export function decodeWorkerRuntimeEnvelope(message) {
    const bytes = cloneBytes(message, "worker runtime message");
    if (bytes.byteLength === 0) {
        throw new TypeError("worker runtime message must not be empty");
    }
    const tag = bytes[0];
    return {
        kind: kindForTag(tag),
        tag,
        bytes,
        payload: bytes.slice(1),
    };
}

export function createPortDescriptor(descriptor, name) {
    return normalizePortDescriptor(descriptor, name);
}

export function openMessagePort(descriptor, options = {}) {
    const messagePortDescriptor = normalizePortDescriptor(descriptor);
    const channel = new MessageChannel();
    const localPort = channel.port1;
    const remotePort = channel.port2;
    const wrap = options.wrap || null;

    return {
        descriptor: messagePortDescriptor,
        localPort,
        remotePort,
        endpoint:
            wrap === "runtime"
                ? new WorkerRuntimeEndpoint(localPort, options.endpoint)
                : wrap === "binary"
                    ? new BinaryMessageEndpoint(localPort, options.endpoint)
                    : null,
    };
}

export function postMessagePort(target, port, options = {}) {
    const messageTarget = requireMessageTarget(target);
    const messagePort = requireMessagePort(port);
    const message = { ...(options.message || {}) };
    const portKey = options.portKey || "port";
    const descriptorKey = options.descriptorKey || "descriptor";

    message[portKey] = messagePort;
    if (options.descriptor != null) {
        message[descriptorKey] = normalizePortDescriptor(options.descriptor);
    }

    messageTarget.postMessage(message, [messagePort]);
    return message;
}

export function takeMessagePort(source, options = {}) {
    const payload = unwrapMessageEvent(source);
    if (!payload || typeof payload !== "object") {
        throw new TypeError("expected a message object containing a transferred MessagePort");
    }

    const portKey = options.portKey || "port";
    const descriptorKey = options.descriptorKey || "descriptor";
    const port = requireMessagePort(payload[portKey]);
    const descriptor =
        payload[descriptorKey] == null
            ? null
            : normalizePortDescriptor(payload[descriptorKey]);

    return {
        port,
        descriptor,
        message: payload,
    };
}

export async function resolveWanixSystemFacade(systemLike) {
    const candidate = await unwrapPromise(systemLike);
    if (!candidate) {
        throw new TypeError("expected a wanix-system element or WanixSystem facade");
    }

    const element = isSystemElementLike(candidate) ? candidate : null;
    if (element?.ready && typeof element.ready.then === "function") {
        await element.ready;
    }

    const facade = element?.system || candidate;
    if (!isSystemFacadeLike(facade)) {
        throw new TypeError("expected a wanix-system element or WanixSystem facade");
    }

    return { element, facade };
}

export async function connectRuntimePort(target, options = {}) {
    const descriptor = normalizePortDescriptor(
        options.descriptor || { port_id: "runtime", name: "runtime" },
    );
    const system =
        options.system == null ? { element: null, facade: null } : await resolveWanixSystemFacade(options.system);
    const { localPort, remotePort } = openMessagePort(descriptor);
    const endpoint = new WorkerRuntimeEndpoint(localPort, options.endpoint);
    const bootstrap = {
        ...(options.message || {}),
        type: options.type || DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
    };

    const taskId = options.task_id ?? options.taskId;
    const workerId = options.worker_id ?? options.workerId;

    if (taskId != null) {
        bootstrap.task_id = String(taskId);
    }
    if (workerId != null) {
        bootstrap.worker_id = String(workerId);
    }
    if (system.element?.id) {
        bootstrap.system_id = system.element.id;
    }

    postMessagePort(target, remotePort, {
        descriptor,
        message: bootstrap,
        portKey: options.portKey,
        descriptorKey: options.descriptorKey,
    });

    const bridge =
        system.facade &&
        typeof system.facade.handleRuntimeRequest === "function" &&
        options.wireToSystem !== false
            ? wireRuntimeEndpointToSystem(endpoint, system.facade, options.bridge)
            : null;

    return {
        descriptor,
        endpoint,
        port: localPort,
        bootstrap,
        bridge,
        element: system.element,
        facade: system.facade,
    };
}

export function acceptRuntimePort(source, options = {}) {
    const { port, descriptor, message } = takeMessagePort(source, options);
    return {
        descriptor,
        port,
        message,
        endpoint: new WorkerRuntimeEndpoint(port, options.endpoint),
    };
}

export async function wireWorkerRuntimeToSystem(worker, system, options = {}) {
    return connectRuntimePort(worker, { ...options, system });
}

export function acceptWorkerRuntime(source, options = {}) {
    return acceptRuntimePort(source, options);
}

export function wireRuntimeEndpointToSystem(endpoint, facade, options = {}) {
    if (!endpoint || typeof endpoint.onRequest !== "function" || typeof endpoint.sendResponse !== "function") {
        throw new TypeError("expected a WorkerRuntimeEndpoint-like object");
    }
    if (!facade || typeof facade.handleRuntimeRequest !== "function") {
        throw new TypeError("expected a WanixSystem facade with handleRuntimeRequest(bytes)");
    }

    const onRequest = endpoint.onRequest((message) => {
        try {
            endpoint.sendResponse(facade.handleRuntimeRequest(message.payload));
        } catch (error) {
            emitListeners(errorListeners, error);
        }
    });

    const onTaskMessage =
        typeof facade.handleRuntimeTaskMessage === "function"
            ? endpoint.onTaskMessage((message) => {
                try {
                    const response = facade.handleRuntimeTaskMessage(message.payload);
                    if (options.respondToTaskMessages === true) {
                        endpoint.sendResponse(response);
                    }
                } catch (error) {
                    emitListeners(errorListeners, error);
                }
            })
            : () => {};

    const errorListeners = new Set();
    if (typeof options.onerror === "function") {
        errorListeners.add(options.onerror);
    }

    return {
        onError(listener) {
            return addListener(errorListeners, listener, "runtime bridge error listener");
        },
        close() {
            onRequest();
            onTaskMessage();
            errorListeners.clear();
        },
    };
}

export function requestWanixImportPort(src, options = {}) {
    if (typeof document === "undefined") {
        throw new Error("requestWanixImportPort requires a browser document");
    }

    const targetOrigin = options.targetOrigin || "*";
    const request = options.request || "wanix-import";
    const url = String(src);
    const timeoutMs = Number(options.timeoutMs ?? options.timeout ?? 5000);
    const retryMs = Math.max(0, Number(options.retryMs ?? options.retryIntervalMs ?? 50));

    return new Promise((resolve, reject) => {
        const iframe = document.createElement("iframe");
        const ports = [];
        let retryTimer = null;
        let timeoutTimer = null;
        let settled = false;

        const cleanup = (removeFrame = true) => {
            settled = true;
            if (retryTimer !== null) {
                clearInterval(retryTimer);
                retryTimer = null;
            }
            if (timeoutTimer !== null) {
                clearTimeout(timeoutTimer);
                timeoutTimer = null;
            }
            for (const port of ports) {
                if (typeof port.close === "function") {
                    port.close();
                }
            }
            if (removeFrame) {
                iframe.remove();
            }
        };

        const resolveOnce = (value) => {
            if (settled) {
                return;
            }
            cleanup(false);
            resolve(attachImportFrameLifetime(value, iframe));
        };

        const rejectOnce = (error) => {
            if (settled) {
                return;
            }
            cleanup();
            reject(error);
        };

        const sendRequest = () => {
            if (settled || !iframe.contentWindow) {
                return;
            }
            const channel = new MessageChannel();
            ports.push(channel.port1);
            channel.port1.onmessage = (event) => {
                try {
                    resolveOnce(requireMessagePort(event.data));
                } catch (error) {
                    rejectOnce(error);
                }
            };
            if (typeof channel.port1.start === "function") {
                channel.port1.start();
            }
            try {
                iframe.contentWindow.postMessage(
                    {
                        request,
                        responder: channel.port2,
                    },
                    targetOrigin,
                    [channel.port2],
                );
            } catch (error) {
                rejectOnce(error);
            }
        };

        iframe.style.display = "none";
        iframe.src = new URL(url, document.baseURI).href;
        iframe.onload = () => {
            if (timeoutMs >= 0) {
                timeoutTimer = setTimeout(() => {
                    rejectOnce(new Error(`timed out waiting for Wanix import port from ${JSON.stringify(url)}`));
                }, timeoutMs);
            }
            sendRequest();
            if (retryMs > 0) {
                retryTimer = setInterval(sendRequest, retryMs);
            }
        };
        iframe.onerror = () => {
            rejectOnce(new Error(`Failed to load import binding source ${JSON.stringify(url)}`));
        };
        document.body.append(iframe);
    });
}

function attachImportFrameLifetime(port, iframe) {
    const messagePort = requireMessagePort(port);
    const close = typeof messagePort.close === "function" ? messagePort.close.bind(messagePort) : null;
    let closed = false;
    messagePort.close = () => {
        if (closed) {
            return;
        }
        closed = true;
        iframe.remove();
        close?.();
    };
    return messagePort;
}

export function createWanixRuntimeNamespaceClient(endpoint, options = {}) {
    if (!endpoint || typeof endpoint.sendRequest !== "function" || typeof endpoint.onResponse !== "function") {
        throw new TypeError("expected a WorkerRuntimeEndpoint-like object");
    }
    const encode = options.encodeRequest || options.encode;
    const decode = options.decodeResponse || options.decode || ((payload) => payload);
    if (typeof encode !== "function") {
        throw new TypeError("expected encodeRequest(request) for runtime namespace requests");
    }
    const taskId = String(options.taskId ?? options.task_id ?? "");
    const timeoutMs = Number(options.timeoutMs ?? options.timeout ?? 5000);
    const pending = [];

    const offResponse = endpoint.onResponse((message) => {
        const next = pending.shift();
        if (!next) {
            return;
        }
        clearTimeout(next.timer);
        try {
            next.resolve(decode(message.payload));
        } catch (error) {
            next.reject(error);
        }
    });
    const offError =
        typeof endpoint.onError === "function"
            ? endpoint.onError((error) => {
                const next = pending.shift();
                if (!next) {
                    return;
                }
                clearTimeout(next.timer);
                next.reject(error);
            })
            : () => {};

    function request(method, args) {
        const payload = encode({
            method,
            args: normalizeRuntimeArgs(args, taskId),
        });
        return new Promise((resolve, reject) => {
            const timer =
                timeoutMs >= 0
                    ? setTimeout(() => {
                        const index = pending.findIndex((entry) => entry.resolve === resolve);
                        if (index >= 0) {
                            pending.splice(index, 1);
                        }
                        reject(new Error(`timed out waiting for Wanix runtime ${method} response`));
                    }, timeoutMs)
                    : null;
            pending.push({ resolve, reject, timer });
            try {
                endpoint.sendRequest(payload);
            } catch (error) {
                if (timer !== null) {
                    clearTimeout(timer);
                }
                pending.pop();
                reject(error);
            }
        });
    }

    return {
        request,
        pathRead(path) {
            return request("PathRead", { path });
        },
        pathWrite(path, data) {
            return request("PathWrite", { path, data: cloneBytes(data, "runtime path write data") });
        },
        pathStat(path) {
            return request("PathStat", { path });
        },
        pathList(path) {
            return request("PathList", { path });
        },
        pathMkdir(path) {
            return request("PathMkdir", { path });
        },
        pathRemove(path) {
            return request("PathRemove", { path });
        },
        pathRename(oldPath, newPath) {
            return request("PathRename", { old_path: String(oldPath), new_path: String(newPath) });
        },
        pathTruncate(path, size) {
            return request("PathTruncate", { path, size: Number(size) });
        },
        fdOpen(path, flags = {}) {
            return request("FdOpen", {
                path,
                read: Boolean(flags.read),
                write: Boolean(flags.write),
                create: Boolean(flags.create),
                truncate: Boolean(flags.truncate),
                append: Boolean(flags.append),
            });
        },
        fdRead(fd, len) {
            return request("FdRead", { fd: Number(fd), len: Number(len) });
        },
        fdWrite(fd, data) {
            return request("FdWrite", { fd: Number(fd), data: cloneBytes(data, "runtime fd write data") });
        },
        fdSeek(fd, offset, whence = "start") {
            return request("FdSeek", { fd: Number(fd), offset: Number(offset), whence: String(whence) });
        },
        fdClose(fd) {
            return request("FdClose", { fd: Number(fd) });
        },
        close() {
            offResponse();
            offError();
            while (pending.length > 0) {
                const next = pending.shift();
                clearTimeout(next.timer);
                next.reject(new Error("Wanix runtime namespace client closed"));
            }
        },
    };
}

function normalizeRuntimeArgs(args, taskId) {
    const value = { ...(args || {}) };
    if (value.task_id == null && value.taskId == null) {
        value.task_id = taskId;
    }
    if (value.taskId != null && value.task_id == null) {
        value.task_id = String(value.taskId);
        delete value.taskId;
    }
    if (typeof value.path !== "undefined") {
        value.path = String(value.path);
    }
    return value;
}

export class BinaryMessageEndpoint {
    constructor(target, options = {}) {
        this.target = requireMessageTarget(target);
        this._listeners = new Set();
        this._errorListeners = new Set();
        this._started = false;
        this._transfer = options.transfer !== false;
        this._handleMessage = this._handleMessage.bind(this);

        if (typeof options.onmessage === "function") {
            this.onMessage(options.onmessage);
        }
        if (typeof options.onerror === "function") {
            this.onError(options.onerror);
        }
        if (options.autoStart !== false) {
            this.start();
        }
    }

    onMessage(listener) {
        return addListener(this._listeners, listener, "binary message listener");
    }

    onError(listener) {
        return addListener(this._errorListeners, listener, "binary message error listener");
    }

    post(bytes) {
        return postBinaryMessage(this.target, bytes, this._transfer);
    }

    start() {
        if (this._started) {
            return this;
        }
        this.target.addEventListener("message", this._handleMessage);
        startMessageTarget(this.target);
        this._started = true;
        return this;
    }

    stop() {
        if (!this._started) {
            return this;
        }
        this.target.removeEventListener("message", this._handleMessage);
        this._started = false;
        return this;
    }

    _handleMessage(event) {
        try {
            const bytes = cloneBytes(unwrapMessageEvent(event), "binary runtime message");
            emitListeners(this._listeners, {
                bytes,
                event,
                target: this.target,
            });
        } catch (error) {
            emitListeners(this._errorListeners, error);
        }
    }
}

export class WorkerRuntimeEndpoint {
    constructor(target, options = {}) {
        this.binary = new BinaryMessageEndpoint(target, {
            autoStart: false,
            transfer: options.transfer,
        });
        this.target = this.binary.target;
        this._listeners = new Set();
        this._requestListeners = new Set();
        this._responseListeners = new Set();
        this._taskListeners = new Set();
        this._errorListeners = new Set();

        this.binary.onMessage((message) => {
            this._handleBinaryMessage(message);
        });
        this.binary.onError((error) => {
            emitListeners(this._errorListeners, error);
        });

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
        if (typeof options.onerror === "function") {
            this.onError(options.onerror);
        }
        if (options.autoStart !== false) {
            this.start();
        }
    }

    onMessage(listener) {
        return addListener(this._listeners, listener, "worker runtime listener");
    }

    onRequest(listener) {
        return addListener(this._requestListeners, listener, "worker runtime request listener");
    }

    onResponse(listener) {
        return addListener(this._responseListeners, listener, "worker runtime response listener");
    }

    onTaskMessage(listener) {
        return addListener(this._taskListeners, listener, "worker runtime task listener");
    }

    onError(listener) {
        return addListener(this._errorListeners, listener, "worker runtime error listener");
    }

    send(kind, payload) {
        return this.binary.post(encodeWorkerRuntimeEnvelope(kind, payload));
    }

    sendRequest(payload) {
        return this.send("request", payload);
    }

    sendResponse(payload) {
        return this.send("response", payload);
    }

    sendTaskMessage(payload) {
        return this.send("task", payload);
    }

    start() {
        this.binary.start();
        return this;
    }

    stop() {
        this.binary.stop();
        return this;
    }

    _handleBinaryMessage(message) {
        try {
            const runtimeMessage = {
                ...decodeWorkerRuntimeEnvelope(message.bytes),
                event: message.event,
                target: this.target,
            };
            emitListeners(this._listeners, runtimeMessage);
            switch (runtimeMessage.kind) {
            case "request":
                emitListeners(this._requestListeners, runtimeMessage);
                break;
            case "response":
                emitListeners(this._responseListeners, runtimeMessage);
                break;
            case "task":
                emitListeners(this._taskListeners, runtimeMessage);
                break;
            }
        } catch (error) {
            emitListeners(this._errorListeners, error);
        }
    }
}

function tagForKind(kind) {
    const normalized = String(kind).trim().toLowerCase();
    const tag = TAG_BY_KIND[normalized];
    if (tag == null) {
        throw new TypeError(`Unsupported worker runtime message kind ${JSON.stringify(kind)}`);
    }
    return tag;
}

function kindForTag(tag) {
    const kind = KIND_BY_TAG[tag];
    if (!kind) {
        throw new TypeError(`Unsupported worker runtime message tag ${tag}`);
    }
    return kind;
}

function postBinaryMessage(target, bytes, transfer) {
    const messageTarget = requireMessageTarget(target);
    const payload = cloneBytes(bytes, "binary runtime payload");
    messageTarget.postMessage(payload, transfer ? [payload.buffer] : []);
    return payload.byteLength;
}

function normalizePortDescriptor(descriptor, fallbackName) {
    if (typeof descriptor === "string") {
        return normalizePortDescriptor({ port_id: descriptor, name: fallbackName });
    }
    if (!descriptor || typeof descriptor !== "object") {
        throw new TypeError("expected a port descriptor object or port id string");
    }

    const port_id = String(descriptor.port_id ?? descriptor.portId ?? "").trim();
    if (!port_id) {
        throw new TypeError("port descriptor must include a non-empty port_id");
    }

    const name = String(descriptor.name ?? fallbackName ?? port_id).trim();
    if (!name) {
        throw new TypeError("port descriptor must include a non-empty name");
    }

    return { port_id, name };
}

function cloneBytes(value, label) {
    const bytes = toUint8Array(value, label);
    return bytes.slice();
}

function toUint8Array(value, label) {
    if (value instanceof Uint8Array) {
        return value;
    }
    if (typeof ArrayBuffer !== "undefined" && value instanceof ArrayBuffer) {
        return new Uint8Array(value);
    }
    if (typeof SharedArrayBuffer !== "undefined" && value instanceof SharedArrayBuffer) {
        return new Uint8Array(value);
    }
    if (ArrayBuffer.isView(value)) {
        return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    throw new TypeError(`expected ${label} to be binary data`);
}

function requireMessageTarget(target) {
    if (!target || typeof target.postMessage !== "function") {
        throw new TypeError("expected a Worker, MessagePort, or other postMessage target");
    }
    if (
        typeof target.addEventListener !== "function" ||
        typeof target.removeEventListener !== "function"
    ) {
        throw new TypeError("message target must support addEventListener/removeEventListener");
    }
    return target;
}

function requireMessagePort(port) {
    if (
        !port ||
        typeof port.postMessage !== "function" ||
        typeof port.start !== "function" ||
        typeof port.close !== "function"
    ) {
        throw new TypeError("expected a MessagePort instance");
    }
    return port;
}

function unwrapMessageEvent(source) {
    if (source && typeof source === "object" && "data" in source) {
        return source.data;
    }
    return source;
}

async function unwrapPromise(value) {
    if (value && typeof value.then === "function") {
        return value;
    }
    return value;
}

function isSystemElementLike(value) {
    return !!value && typeof value === "object" && "system" in value;
}

function isSystemFacadeLike(value) {
    return (
        !!value &&
        typeof value.readText === "function" &&
        typeof value.writeText === "function" &&
        typeof value.setupNamespace === "function"
    );
}

function startMessageTarget(target) {
    if (typeof target.start === "function") {
        target.start();
    }
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
