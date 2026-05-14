const VERSION = "9P2000.L";
const DEFAULT_MSIZE = 64 * 1024;
const NOFID = 0xffffffff;
const ATTR_BASIC = 0x67f;
const AT_REMOVEDIR = 0x200;
const ERROR_EIO = 5;
const ERROR_ENOENT = 2;
const ERROR_EEXIST = 17;
const ERROR_ENOTDIR = 20;
const ERROR_EISDIR = 21;
const ERROR_EINVAL = 22;
const ERROR_ENOTSUP = 58;
const DT_DIR = 4;
const DT_REG = 8;

const MSG = Object.freeze({
    RLERROR: 7,
    TLOPEN: 12,
    RLOPEN: 13,
    TLCREATE: 14,
    RLCREATE: 15,
    TGETATTR: 24,
    RGETATTR: 25,
    TREADDIR: 40,
    RREADDIR: 41,
    TMKDIR: 72,
    RMKDIR: 73,
    TUNLINKAT: 76,
    RUNLINKAT: 77,
    TVERSION: 100,
    RVERSION: 101,
    TATTACH: 104,
    RATTACH: 105,
    TFLUSH: 108,
    RFLUSH: 109,
    TWALK: 110,
    RWALK: 111,
    TREAD: 116,
    RREAD: 117,
    TWRITE: 118,
    RWRITE: 119,
    TCLUNK: 120,
    RCLUNK: 121,
});

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

export function createStorageP9FramePort(adapter, options = {}) {
    if (typeof MessageChannel !== "function") {
        throw new Error("createStorageP9FramePort requires MessageChannel");
    }
    const channel = new MessageChannel();
    const server = serveStorageP9FramePort(channel.port1, adapter, {
        ...options,
        ownsTarget: true,
    });
    return {
        adapter,
        localPort: channel.port1,
        port: channel.port2,
        server,
    };
}

export function serveStorageP9FramePort(target, adapter, options = {}) {
    return new StorageP9FrameServer(target, adapter, options).start();
}

export class StorageP9FrameServer {
    constructor(target, adapter, options = {}) {
        this.target = requireMessagePortLike(target);
        this.adapter = requireStorageAdapter(adapter);
        this.msize = Number(options.msize || DEFAULT_MSIZE);
        this.closed = false;
        this.started = false;
        this._ownsTarget = options.ownsTarget === true;
        this._fids = new Map();
        this._pending = new Map();
        this._errorListeners = new Set();
        this._handleMessage = this._handleMessage.bind(this);

        if (typeof options.onerror === "function") {
            this.onError(options.onerror);
        }
    }

    onError(listener) {
        return addListener(this._errorListeners, listener, "storage 9P error listener");
    }

    start() {
        if (this.closed || this.started) {
            return this;
        }
        this.target.addEventListener("message", this._handleMessage);
        this.target.start();
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
        for (const pending of this._pending.values()) {
            pending.controller.abort(new Error("storage 9P server closed"));
        }
        this._pending.clear();
        this._fids.clear();
        this.stop();
        if (this._ownsTarget && typeof this.target.close === "function") {
            this.target.close();
        }
        this.closed = true;
        return this;
    }

    _handleMessage(event) {
        let frame = null;
        let tag = 0xffff;
        try {
            frame = toUint8Array(event?.data, "9P request frame");
            const reader = new Reader(frame);
            const size = reader.u32();
            if (size !== frame.byteLength) {
                throw storageP9Error(ERROR_EINVAL, "invalid 9P frame size");
            }
            const type = reader.u8();
            tag = reader.u16();

            if (type === MSG.TFLUSH) {
                const oldtag = reader.u16();
                this._pending.get(oldtag)?.controller.abort(new Error(`9P request tag ${oldtag} flushed`));
                this._pending.delete(oldtag);
                this.target.postMessage(frameFrom(MSG.RFLUSH, tag));
                return;
            }

            const existing = tag !== 0xffff ? this._pending.get(tag) : null;
            if (existing) {
                existing.controller.abort(new Error(`duplicate 9P request tag ${tag}`));
                this._pending.delete(tag);
                throw storageP9Error(ERROR_EINVAL, `duplicate storage 9P request tag ${tag}`);
            }
            const controller = new AbortController();
            this._pending.set(tag, { controller });
            Promise.resolve(this._process(type, tag, reader, controller.signal)).then(
                (response) => {
                    if (controller.signal.aborted || this._pending.get(tag)?.controller !== controller) {
                        return;
                    }
                    this._pending.delete(tag);
                    this.target.postMessage(response);
                },
                (error) => {
                    if (controller.signal.aborted || this._pending.get(tag)?.controller !== controller) {
                        return;
                    }
                    this._pending.delete(tag);
                    this.target.postMessage(errorFrame(tag, error));
                    emitListeners(this._errorListeners, error);
                },
            );
        } catch (error) {
            this.target.postMessage(errorFrame(tag, error));
            emitListeners(this._errorListeners, error);
        }
    }

    async _process(type, tag, reader, signal) {
        const out = new Writer();
        switch (type) {
        case MSG.TVERSION: {
            const requestedMsize = reader.u32();
            const version = reader.string();
            if (version !== VERSION) {
                throw storageP9Error(ERROR_ENOTSUP, `unsupported 9P version ${version}`);
            }
            this.msize = Math.max(1024, Math.min(requestedMsize, this.msize));
            out.u32(this.msize);
            out.string(VERSION);
            return frameFrom(MSG.RVERSION, tag, out.bytes());
        }
        case MSG.TATTACH: {
            const fid = reader.u32();
            reader.u32();
            reader.string();
            reader.string();
            reader.u32();
            this._fids.set(fid, ".");
            out.qid(".", "dir");
            return frameFrom(MSG.RATTACH, tag, out.bytes());
        }
        case MSG.TWALK:
            return this._walk(tag, reader, signal);
        case MSG.TLOPEN: {
            const fid = reader.u32();
            reader.u32();
            const path = this._fidPath(fid);
            const stat = await this.adapter.stat(path, { signal });
            out.qid(path, kindOf(stat));
            out.u32(8192);
            return frameFrom(MSG.RLOPEN, tag, out.bytes());
        }
        case MSG.TLCREATE:
            return this._create(tag, reader, signal);
        case MSG.TGETATTR:
            return this._getattr(tag, reader, signal);
        case MSG.TREADDIR:
            return this._readdir(tag, reader, signal);
        case MSG.TREAD:
            return this._read(tag, reader, signal);
        case MSG.TWRITE:
            return this._write(tag, reader, signal);
        case MSG.TMKDIR:
            return this._mkdir(tag, reader, signal);
        case MSG.TUNLINKAT:
            return this._unlinkat(tag, reader, signal);
        case MSG.TCLUNK:
            this._fids.delete(reader.u32());
            return frameFrom(MSG.RCLUNK, tag);
        default:
            throw storageP9Error(ERROR_ENOTSUP, `unsupported storage 9P request type ${type}`);
        }
    }

    async _walk(tag, reader, signal) {
        const fid = reader.u32();
        const newfid = reader.u32();
        const count = reader.u16();
        let path = this._fidPath(fid);
        const qids = [];
        for (let index = 0; index < count; index += 1) {
            const name = reader.string();
            const next = joinPath(path, name);
            try {
                const stat = await this.adapter.stat(next, { signal });
                path = next;
                qids.push({ path, kind: kindOf(stat) });
            } catch {
                break;
            }
        }
        if (qids.length === count) {
            this._fids.set(newfid, path);
        }
        const out = new Writer();
        out.u16(qids.length);
        for (const qid of qids) {
            out.qid(qid.path, qid.kind);
        }
        return frameFrom(MSG.RWALK, tag, out.bytes());
    }

    async _create(tag, reader, signal) {
        const fid = reader.u32();
        const name = reader.string();
        reader.u32();
        reader.u32();
        reader.u32();
        const path = joinPath(this._fidPath(fid), name);
        await this.adapter.writeFile(path, new Uint8Array(), { signal });
        this._fids.set(fid, path);
        const out = new Writer();
        out.qid(path, "file");
        out.u32(8192);
        return frameFrom(MSG.RLCREATE, tag, out.bytes());
    }

    async _getattr(tag, reader, signal) {
        const fid = reader.u32();
        reader.u64();
        const path = this._fidPath(fid);
        const stat = await this.adapter.stat(path, { signal });
        const out = new Writer();
        out.u64(ATTR_BASIC);
        out.qid(path, kindOf(stat));
        out.u32(kindOf(stat) === "dir" ? 0o040755 : 0o100644);
        out.u32(0);
        out.u32(0);
        out.u64(1);
        out.u64(0);
        out.u64(Number(stat.size || 0));
        out.u64(4096);
        out.u64(Math.ceil(Number(stat.size || 0) / 4096));
        for (let index = 0; index < 10; index += 1) {
            out.u64(0);
        }
        return frameFrom(MSG.RGETATTR, tag, out.bytes());
    }

    async _readdir(tag, reader, signal) {
        const path = this._fidPath(reader.u32());
        const offset = Number(reader.u64());
        const count = reader.u32();
        const entries = await this.adapter.readDir(path, { signal });
        const body = new Writer();
        let bodyLength = 0;
        const sorted = Array.from(entries).sort((left, right) => String(left.name).localeCompare(String(right.name)));
        for (let index = offset; index < sorted.length; index += 1) {
            const entry = sorted[index];
            const kind = kindOf(entry);
            const dirent = new Writer();
            dirent.qid(joinPath(path, entry.name), kind);
            dirent.u64(index + 1);
            dirent.u8(kind === "dir" ? DT_DIR : DT_REG);
            dirent.string(entry.name);
            const bytes = dirent.bytes();
            if (bodyLength > 0 && bodyLength + bytes.byteLength > count) {
                break;
            }
            if (bytes.byteLength > count) {
                break;
            }
            body.raw(bytes);
            bodyLength += bytes.byteLength;
        }
        const out = new Writer();
        out.countedData(body.bytes());
        return frameFrom(MSG.RREADDIR, tag, out.bytes());
    }

    async _read(tag, reader, signal) {
        const path = this._fidPath(reader.u32());
        const offset = Number(reader.u64());
        const count = reader.u32();
        const data = await this.adapter.readFile(path, { signal });
        const out = new Writer();
        out.countedData(toUint8Array(data, "storage read bytes").slice(offset, offset + count));
        return frameFrom(MSG.RREAD, tag, out.bytes());
    }

    async _write(tag, reader, signal) {
        const path = this._fidPath(reader.u32());
        const offset = Number(reader.u64());
        const data = reader.countedData();
        let current = new Uint8Array();
        try {
            current = toUint8Array(await this.adapter.readFile(path, { signal }), "storage write current bytes");
        } catch {
            current = new Uint8Array();
        }
        const next = new Uint8Array(Math.max(current.byteLength, offset + data.byteLength));
        next.set(current);
        next.set(data, offset);
        await this.adapter.writeFile(path, next, { signal });
        const out = new Writer();
        out.u32(data.byteLength);
        return frameFrom(MSG.RWRITE, tag, out.bytes());
    }

    async _mkdir(tag, reader, signal) {
        const parent = this._fidPath(reader.u32());
        const name = reader.string();
        reader.u32();
        reader.u32();
        await this.adapter.mkdir(joinPath(parent, name), { signal });
        const out = new Writer();
        out.qid(joinPath(parent, name), "dir");
        return frameFrom(MSG.RMKDIR, tag, out.bytes());
    }

    async _unlinkat(tag, reader, signal) {
        const parent = this._fidPath(reader.u32());
        const name = reader.string();
        reader.u32();
        await this.adapter.remove(joinPath(parent, name), { signal });
        return frameFrom(MSG.RUNLINKAT, tag);
    }

    _fidPath(fid) {
        const path = this._fids.get(fid);
        if (!path) {
            throw storageP9Error(ERROR_EINVAL, `unknown fid ${fid}`);
        }
        return path;
    }
}

class Writer {
    constructor() {
        this.out = [];
    }

    u8(value) {
        this.out.push(Number(value) & 0xff);
        return this;
    }

    u16(value) {
        this.out.push(Number(value) & 0xff, (Number(value) >> 8) & 0xff);
        return this;
    }

    u32(value) {
        const number = Number(value) >>> 0;
        this.out.push(number & 0xff, (number >> 8) & 0xff, (number >> 16) & 0xff, (number >> 24) & 0xff);
        return this;
    }

    u64(value) {
        let bigint = BigInt(value);
        for (let index = 0; index < 8; index += 1) {
            this.out.push(Number((bigint >> BigInt(index * 8)) & 0xffn));
        }
        return this;
    }

    string(value) {
        const bytes = textEncoder.encode(String(value));
        this.u16(bytes.byteLength);
        this.raw(bytes);
        return this;
    }

    countedData(value) {
        const bytes = toUint8Array(value, "9P counted data");
        this.u32(bytes.byteLength);
        this.raw(bytes);
        return this;
    }

    qid(path, kind = "file") {
        this.u8(kind === "dir" ? 0x80 : 0);
        this.u32(0);
        this.u64(fnv1a64(path));
        return this;
    }

    raw(value) {
        this.out.push(...toUint8Array(value, "9P raw bytes"));
        return this;
    }

    bytes() {
        return Uint8Array.from(this.out);
    }
}

class Reader {
    constructor(bytes) {
        this.bytes = toUint8Array(bytes, "9P request frame");
        this.offset = 0;
    }

    u8() {
        this._require(1);
        return this.bytes[this.offset++];
    }

    u16() {
        this._require(2);
        const value = this.bytes[this.offset] | (this.bytes[this.offset + 1] << 8);
        this.offset += 2;
        return value;
    }

    u32() {
        this._require(4);
        const value =
            this.bytes[this.offset] |
            (this.bytes[this.offset + 1] << 8) |
            (this.bytes[this.offset + 2] << 16) |
            (this.bytes[this.offset + 3] << 24);
        this.offset += 4;
        return value >>> 0;
    }

    u64() {
        this._require(8);
        let value = 0n;
        for (let index = 0; index < 8; index += 1) {
            value |= BigInt(this.bytes[this.offset + index]) << BigInt(index * 8);
        }
        this.offset += 8;
        return Number(value);
    }

    string() {
        const length = this.u16();
        this._require(length);
        const value = textDecoder.decode(this.bytes.slice(this.offset, this.offset + length));
        this.offset += length;
        return value;
    }

    countedData() {
        const length = this.u32();
        this._require(length);
        const value = this.bytes.slice(this.offset, this.offset + length);
        this.offset += length;
        return value;
    }

    _require(length) {
        if (this.offset + length > this.bytes.byteLength) {
            throw storageP9Error(ERROR_EINVAL, "truncated 9P request");
        }
    }
}

function frameFrom(type, tag, payload = new Uint8Array()) {
    const body = toUint8Array(payload, "9P response payload");
    const frame = new Uint8Array(7 + body.byteLength);
    frame[0] = frame.byteLength & 0xff;
    frame[1] = (frame.byteLength >> 8) & 0xff;
    frame[2] = (frame.byteLength >> 16) & 0xff;
    frame[3] = (frame.byteLength >> 24) & 0xff;
    frame[4] = type & 0xff;
    frame[5] = tag & 0xff;
    frame[6] = (tag >> 8) & 0xff;
    frame.set(body, 7);
    return frame;
}

function errorFrame(tag, error) {
    const out = new Writer();
    out.u32(errnoFor(error));
    return frameFrom(MSG.RLERROR, tag, out.bytes());
}

function errnoFor(error) {
    const code = String(error?.code || "").toUpperCase();
    if (code === "ENOENT" || code === "NOTFOUND") return ERROR_ENOENT;
    if (code === "EEXIST") return ERROR_EEXIST;
    if (code === "ENOTDIR") return ERROR_ENOTDIR;
    if (code === "EISDIR") return ERROR_EISDIR;
    if (code === "EINVAL") return ERROR_EINVAL;
    if (code === "ENOTSUP" || code === "ENOTSUPPORTED" || code === "NOTSUPPORTED") return ERROR_ENOTSUP;
    return Number(error?.errno || error?.ecode || ERROR_EIO);
}

function storageP9Error(errno, message) {
    const error = new Error(message);
    error.errno = errno;
    return error;
}

function kindOf(stat) {
    const value = stat?.kind || stat?.type;
    return value === "dir" || value === "directory" ? "dir" : "file";
}

function joinPath(parent = ".", child = ".") {
    const cleanParent = normalizePath(parent);
    const cleanChild = normalizePath(child);
    if (cleanParent === ".") {
        return cleanChild;
    }
    if (cleanChild === ".") {
        return cleanParent;
    }
    return `${cleanParent}/${cleanChild}`;
}

function normalizePath(path) {
    if (path == null || path === "" || path === ".") {
        return ".";
    }
    const value = String(path);
    if (value.startsWith("/") || value.includes("\\")) {
        throw storageP9Error(ERROR_EINVAL, `invalid storage path ${JSON.stringify(path)}`);
    }
    const parts = [];
    for (const part of value.split("/")) {
        if (!part || part === ".") {
            continue;
        }
        if (part === "..") {
            throw storageP9Error(ERROR_EINVAL, `path traversal is not allowed: ${JSON.stringify(path)}`);
        }
        parts.push(part);
    }
    return parts.length === 0 ? "." : parts.join("/");
}

function fnv1a64(value) {
    let hash = 0xcbf29ce484222325n;
    for (const byte of textEncoder.encode(String(value))) {
        hash ^= BigInt(byte);
        hash = (hash * 0x100000001b3n) & 0xffffffffffffffffn;
    }
    return hash;
}

function requireStorageAdapter(adapter) {
    if (!adapter || typeof adapter !== "object") {
        throw new TypeError("expected a storage adapter object");
    }
    for (const method of ["stat", "readFile", "writeFile", "readDir", "mkdir", "remove"]) {
        if (typeof adapter[method] !== "function") {
            throw new TypeError(`storage adapter must implement ${method}()`);
        }
    }
    return adapter;
}

function requireMessagePortLike(port) {
    if (
        !port ||
        typeof port.postMessage !== "function" ||
        typeof port.addEventListener !== "function" ||
        typeof port.removeEventListener !== "function" ||
        typeof port.start !== "function"
    ) {
        throw new TypeError("expected a MessagePort-like storage 9P target");
    }
    return port;
}

function addListener(listeners, listener, label) {
    if (typeof listener !== "function") {
        throw new TypeError(`${label} must be a function`);
    }
    listeners.add(listener);
    return () => listeners.delete(listener);
}

function emitListeners(listeners, value) {
    for (const listener of listeners) {
        listener(value);
    }
}

function toUint8Array(value, label = "bytes") {
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
    throw new TypeError(`expected ${label} to be binary data`);
}
