import { BinaryMessageEndpoint } from "./worker-runtime.js";

export const DEFAULT_WANIX_IMPORT_REQUEST = "wanix-import";

export async function resolveWanixP9Facade(systemLike) {
    const candidate = await systemLike;
    if (!candidate) {
        throw new TypeError("expected a wanix-system element or WanixSystem facade");
    }

    const element = isSystemElementLike(candidate) ? candidate : null;
    if (element?.ready && typeof element.ready.then === "function") {
        await element.ready;
    }

    const facade = element?.system || candidate;
    if (!isWanixP9FacadeLike(facade)) {
        throw new TypeError("expected a wanix-system element or WanixSystem facade with handle9pFrame(frame)");
    }

    return { element, facade };
}

export async function serveWanixP9FramePort(target, systemLike, options = {}) {
    const { element, facade } = await resolveWanixP9Facade(systemLike);
    return new WanixP9FramePortServer(target, facade, {
        ...options,
        element,
    });
}

export async function createWanixP9FramePort(systemLike, options = {}) {
    requireMessageChannel();

    const { element, facade } = await resolveWanixP9Facade(systemLike);
    const channel = new MessageChannel();
    const server = new WanixP9FramePortServer(channel.port1, facade, {
        ...options,
        element,
        ownsTarget: true,
    });

    return {
        element,
        facade,
        localPort: channel.port1,
        port: channel.port2,
        server,
    };
}

export async function attachWanixImportResponder(target, systemLike, options = {}) {
    const { element, facade } = await resolveWanixP9Facade(systemLike);
    return new WanixImportResponder(target, facade, {
        ...options,
        element,
    });
}

export class WanixP9FramePortServer {
    constructor(target, facade, options = {}) {
        this.endpoint = new BinaryMessageEndpoint(target, {
            autoStart: false,
            transfer: options.transfer,
        });
        this.target = this.endpoint.target;
        this.facade = requireWanixP9Facade(facade);
        this.element = options.element || null;
        this.closed = false;
        this.started = false;
        this._ownsTarget = options.ownsTarget === true;
        this._requestListeners = new Set();
        this._responseListeners = new Set();
        this._errorListeners = new Set();

        this.endpoint.onMessage((message) => {
            this._handleMessage(message);
        });
        this.endpoint.onError((error) => {
            emitListeners(this._errorListeners, error);
        });

        if (typeof options.onrequest === "function") {
            this.onRequest(options.onrequest);
        }
        if (typeof options.onresponse === "function") {
            this.onResponse(options.onresponse);
        }
        if (typeof options.onerror === "function") {
            this.onError(options.onerror);
        }
        if (options.autoStart !== false) {
            this.start();
        }
    }

    onRequest(listener) {
        return addListener(this._requestListeners, listener, "9P frame request listener");
    }

    onResponse(listener) {
        return addListener(this._responseListeners, listener, "9P frame response listener");
    }

    onError(listener) {
        return addListener(this._errorListeners, listener, "9P frame error listener");
    }

    start() {
        if (this.closed || this.started) {
            return this;
        }
        this.endpoint.start();
        this.started = true;
        return this;
    }

    stop() {
        if (!this.started) {
            return this;
        }
        this.endpoint.stop();
        this.started = false;
        return this;
    }

    close() {
        if (this.closed) {
            return this;
        }
        this.stop();
        if (this._ownsTarget && typeof this.target.close === "function") {
            this.target.close();
        }
        this.closed = true;
        return this;
    }

    _handleMessage(message) {
        try {
            const request = cloneFrameBytes(message.bytes, "9P request frame");
            emitListeners(this._requestListeners, {
                bytes: request,
                event: message.event,
                target: this.target,
            });

            const response = cloneFrameBytes(
                this.facade.handle9pFrame(request),
                "9P response frame",
            );
            this.endpoint.post(response);

            emitListeners(this._responseListeners, {
                event: message.event,
                request,
                response,
                target: this.target,
            });
        } catch (error) {
            emitListeners(this._errorListeners, error);
        }
    }
}

export class WanixImportResponder {
    constructor(target, facade, options = {}) {
        this.target = requireListenerTarget(target);
        this.facade = requireWanixP9Facade(facade);
        this.element = options.element || null;
        this.request = String(options.request || DEFAULT_WANIX_IMPORT_REQUEST);
        this.closed = false;
        this.started = false;
        this._servers = new Set();
        this._requestListeners = new Set();
        this._errorListeners = new Set();
        this._handleMessage = this._handleMessage.bind(this);

        if (typeof options.onrequest === "function") {
            this.onRequest(options.onrequest);
        }
        if (typeof options.onerror === "function") {
            this.onError(options.onerror);
        }
        if (options.autoStart !== false) {
            this.start();
        }
    }

    onRequest(listener) {
        return addListener(this._requestListeners, listener, "wanix import request listener");
    }

    onError(listener) {
        return addListener(this._errorListeners, listener, "wanix import error listener");
    }

    start() {
        if (this.closed || this.started) {
            return this;
        }
        this.target.addEventListener("message", this._handleMessage);
        this.started = true;
        return this;
    }

    stop() {
        if (!this.started) {
            return this;
        }
        this.target.removeEventListener("message", this._handleMessage);
        this.started = false;
        return this;
    }

    close() {
        if (this.closed) {
            return this;
        }
        this.stop();
        for (const server of this._servers) {
            server.close();
        }
        this._servers.clear();
        this.closed = true;
        return this;
    }

    _handleMessage(event) {
        const payload = unwrapMessageEvent(event);
        if (!isImportRequest(payload, this.request)) {
            return;
        }

        let server = null;
        try {
            requireMessageChannel();
            const responder = requireMessagePort(payload.responder);
            startMessageTarget(responder);

            const channel = new MessageChannel();
            server = new WanixP9FramePortServer(channel.port1, this.facade, {
                ownsTarget: true,
            });
            this._servers.add(server);

            responder.postMessage(channel.port2, [channel.port2]);
            emitListeners(this._requestListeners, {
                event,
                responder,
                port: channel.port2,
                server,
                target: this.target,
            });
        } catch (error) {
            if (server) {
                this._servers.delete(server);
                server.close();
            }
            emitListeners(this._errorListeners, error);
        }
    }
}

function requireWanixP9Facade(facade) {
    if (!isWanixP9FacadeLike(facade)) {
        throw new TypeError("expected a WanixSystem facade with handle9pFrame(frame)");
    }
    return facade;
}

function cloneFrameBytes(value, label) {
    return toUint8Array(value, label).slice();
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

function requireListenerTarget(target) {
    if (
        !target ||
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

function requireMessageChannel() {
    if (typeof MessageChannel !== "function") {
        throw new TypeError("MessageChannel is not available in this environment");
    }
}

function isImportRequest(payload, request) {
    return !!payload && typeof payload === "object" && payload.request === request;
}

function unwrapMessageEvent(source) {
    if (source && typeof source === "object" && "data" in source) {
        return source.data;
    }
    return source;
}

function isSystemElementLike(value) {
    return !!value && typeof value === "object" && "system" in value;
}

function isWanixP9FacadeLike(value) {
    return !!value && typeof value === "object" && typeof value.handle9pFrame === "function";
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
