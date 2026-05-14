const DEFAULT_TEXT_ENCODER = new TextEncoder();
const DEFAULT_TEXT_DECODER = new TextDecoder();

export function createBrowserNetworkDevice(options = {}) {
    return new BrowserNetworkDevice(options);
}

export class BrowserNetworkDevice {
    constructor(options = {}) {
        this._nextId = 0;
        this._connections = new Map();
        this._transportFactory = options.transportFactory || defaultTransportFactory(options);
    }

    create() {
        const id = String(++this._nextId);
        const conn = new BrowserNetworkConnection(id, this._transportFactory);
        this._connections.set(id, conn);
        return conn;
    }

    get(id) {
        return this._connections.get(String(id)) || null;
    }

    list() {
        return [...this._connections.keys()].sort();
    }

    remove(id) {
        const conn = this.get(id);
        if (!conn) {
            return false;
        }
        conn.close();
        this._connections.delete(String(id));
        return true;
    }

    readDir(path = ".") {
        const name = normalizeNetworkPath(path);
        if (name !== ".") {
            const [head, rest = "."] = splitPath(name);
            const conn = this.get(head);
            if (!conn) {
                throw new Error(`network resource not found: ${head}`);
            }
            return conn.readDir(rest);
        }
        return ["new", ...this.list()];
    }

    readFile(path) {
        const name = normalizeNetworkPath(path);
        if (name === "new") {
            return encodeText(`${this.create().id}\n`);
        }
        const [head, rest = "."] = splitPath(name);
        const conn = this.get(head);
        if (!conn) {
            throw new Error(`network resource not found: ${head}`);
        }
        return conn.readFile(rest);
    }

    writeFile(path, data) {
        const name = normalizeNetworkPath(path);
        const [head, rest = "."] = splitPath(name);
        const conn = this.get(head);
        if (!conn) {
            throw new Error(`network resource not found: ${head}`);
        }
        return conn.writeFile(rest, data);
    }
}

export class BrowserNetworkConnection {
    constructor(id, transportFactory) {
        this.id = String(id);
        this._transportFactory = transportFactory;
        this._transport = null;
        this._phase = "idle";
        this._local = "";
        this._remote = "";
        this._lastError = "";
        this._inbound = [];
    }

    status() {
        let status = this._phase;
        if (this._local) {
            status += ` local=${this._local}`;
        }
        if (this._remote) {
            status += ` remote=${this._remote}`;
        }
        if (this._lastError) {
            status += ` err=${this._lastError}`;
        }
        return status;
    }

    readDir(path = ".") {
        const name = normalizeNetworkPath(path);
        if (name !== ".") {
            throw new Error(`network path is not a directory: ${path}`);
        }
        return ["ctl", "data", "id", "local", "remote", "status"];
    }

    readFile(path) {
        switch (normalizeNetworkPath(path)) {
        case ".":
            return encodeText(`${this.readDir(".").join("\n")}\n`);
        case "id":
            return encodeText(`${this.id}\n`);
        case "status":
            return encodeText(`${this.status()}\n`);
        case "local":
            return encodeText(`${this._local}\n`);
        case "remote":
            return encodeText(`${this._remote}\n`);
        case "data":
            return this.readData();
        default:
            throw new Error(`network path not found: ${path}`);
        }
    }

    writeFile(path, data) {
        switch (normalizeNetworkPath(path)) {
        case "ctl":
            this.writeCtl(decodeText(data));
            return byteLength(data);
        case "data":
            return this.writeData(data);
        default:
            throw new Error(`network path is not writable: ${path}`);
        }
    }

    writeCtl(command) {
        const [verb, ...rest] = String(command).trim().split(/\s+/).filter(Boolean);
        const arg = rest.join(" ");
        switch (verb) {
        case "":
        case undefined:
        case "noop":
            return;
        case "dial":
            this.dial(arg);
            return;
        case "hangup":
            this.close();
            return;
        case "reset":
            this.close();
            this._phase = "idle";
            this._remote = "";
            this._local = "";
            this._lastError = "";
            this._inbound.length = 0;
            return;
        case "listen":
        case "announce":
            this._lastError = `${verb}: browser raw listen is unavailable`;
            throw new Error(this._lastError);
        default:
            this._lastError = `ctl: unsupported command ${verb}`;
            throw new Error(this._lastError);
        }
    }

    dial(url) {
        if (!url) {
            this._lastError = "dial: missing URL";
            throw new Error(this._lastError);
        }
        if (this._transport) {
            this.close();
        }
        this._remote = String(url);
        this._phase = "connecting";
        this._lastError = "";
        const transport = this._transportFactory(this._remote, this);
        this._transport = transport;
        installTransportHandlers(transport, {
            open: () => {
                this._phase = "connected";
                this._local = "browser";
                this._lastError = "";
            },
            message: (data) => {
                this._inbound.push(cloneBytes(data, "browser network data"));
            },
            close: () => {
                this._phase = "closed";
            },
            error: (error) => {
                this._phase = "closed";
                this._lastError = error?.message || String(error || "transport error");
            },
        });
        if (transport.readyState === 1) {
            this._phase = "connected";
            this._local = "browser";
        }
    }

    readData() {
        if (this._inbound.length === 0) {
            return new Uint8Array();
        }
        return this._inbound.shift();
    }

    writeData(data) {
        if (!this._transport || this._phase === "closed") {
            this._lastError = "data: unavailable while disconnected";
            throw new Error(this._lastError);
        }
        const bytes = cloneBytes(data, "browser network write data");
        this._transport.send(bytes);
        return bytes.byteLength;
    }

    close() {
        if (this._transport && typeof this._transport.close === "function") {
            this._transport.close();
        }
        this._transport = null;
        this._phase = "closed";
    }
}

function defaultTransportFactory(options) {
    const WebSocketCtor = options.WebSocket || globalThis.WebSocket;
    return (url) => {
        if (typeof WebSocketCtor !== "function") {
            throw new Error("browser WebSocket transport is unavailable");
        }
        const socket = new WebSocketCtor(url);
        socket.binaryType = "arraybuffer";
        return socket;
    };
}

function installTransportHandlers(transport, handlers) {
    if (typeof transport.addEventListener === "function") {
        transport.addEventListener("open", handlers.open);
        transport.addEventListener("message", (event) => handlers.message(event.data));
        transport.addEventListener("close", handlers.close);
        transport.addEventListener("error", (event) => handlers.error(event.error || event));
        return;
    }
    transport.onopen = handlers.open;
    transport.onmessage = (event) => handlers.message(event.data);
    transport.onclose = handlers.close;
    transport.onerror = (event) => handlers.error(event.error || event);
}

function splitPath(path) {
    const index = path.indexOf("/");
    if (index < 0) {
        return [path];
    }
    return [path.slice(0, index), path.slice(index + 1)];
}

function normalizeNetworkPath(path) {
    const parts = String(path || ".")
        .split("/")
        .filter((part) => part && part !== ".");
    return parts.length === 0 ? "." : parts.join("/");
}

function encodeText(value) {
    return DEFAULT_TEXT_ENCODER.encode(String(value));
}

function decodeText(value) {
    if (typeof value === "string") {
        return value;
    }
    return DEFAULT_TEXT_DECODER.decode(cloneBytes(value, "browser network control data"));
}

function byteLength(value) {
    if (typeof value === "string") {
        return DEFAULT_TEXT_ENCODER.encode(value).byteLength;
    }
    return cloneBytes(value, "browser network data").byteLength;
}

function cloneBytes(value, label) {
    if (value instanceof Uint8Array) {
        return new Uint8Array(value);
    }
    if (value instanceof ArrayBuffer) {
        return new Uint8Array(value.slice(0));
    }
    if (ArrayBuffer.isView(value)) {
        return new Uint8Array(value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength));
    }
    if (Array.isArray(value)) {
        return new Uint8Array(value);
    }
    if (typeof value === "string") {
        return encodeText(value);
    }
    throw new TypeError(`${label} must be bytes or text`);
}
