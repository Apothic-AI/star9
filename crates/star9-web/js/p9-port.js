import { BinaryMessageEndpoint } from "./worker-runtime.js";

export const DEFAULT_STAR9_IMPORT_REQUEST = "star9-import";

export async function resolveStar9P9Facade(systemLike) {
    const candidate = await systemLike;
    if (!candidate) {
        throw new TypeError("expected a star9-system element or Star9System facade");
    }

    const element = isSystemElementLike(candidate) ? candidate : null;
    let facade = element ? readElementFacade(element) : candidate;
    if (!isStar9P9FacadeLike(facade) && element?.ready && typeof element.ready.then === "function") {
        await element.ready;
        facade = readElementFacade(element);
    }
    if (!isStar9P9FacadeLike(facade)) {
        throw new TypeError("expected a star9-system element or Star9System facade with handle9pFrame(frame)");
    }

    return { element, facade };
}

export async function serveStar9P9FramePort(target, systemLike, options = {}) {
    const { element, facade } = await resolveStar9P9Facade(systemLike);
    return new Star9P9FramePortServer(target, facade, {
        ...options,
        element,
    });
}

export async function createStar9P9FramePort(systemLike, options = {}) {
    requireMessageChannel();

    const { element, facade } = await resolveStar9P9Facade(systemLike);
    const channel = new MessageChannel();
    const server = new Star9P9FramePortServer(channel.port1, facade, {
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

export function createStar9P9FrameClient(target, options = {}) {
    return new Star9P9FramePortClient(target, options);
}

export async function createStar9P9NamespaceMount(target, options = {}) {
    return Star9P9NamespaceMount.connect(target, options);
}

export async function attachStar9ImportResponder(target, systemLike, options = {}) {
    const { element, facade } = await resolveStar9P9Facade(systemLike);
    return new Star9ImportResponder(target, facade, {
        ...options,
        element,
    });
}

export class Star9P9FramePortServer {
    constructor(target, facade, options = {}) {
        this.endpoint = new BinaryMessageEndpoint(target, {
            autoStart: false,
            transfer: options.transfer,
        });
        this.target = this.endpoint.target;
        this.facade = requireStar9P9Facade(facade);
        this.element = options.element || null;
        this.closed = false;
        this.started = false;
        this._ownsTarget = options.ownsTarget === true;
        this._pending = new Map();
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
        for (const pending of this._pending.values()) {
            pending.controller.abort(new Error("9P frame server closed"));
        }
        this._pending.clear();
        this.stop();
        if (this._ownsTarget && typeof this.target.close === "function") {
            this.target.close();
        }
        this.closed = true;
        return this;
    }

    _handleMessage(message) {
        let request = null;
        try {
            request = cloneFrameBytes(message.bytes, "9P request frame");
            const tag = frameTag(request);
            if (frameType(request) === MSG.TFLUSH) {
                const oldtag = flushOldTag(request);
                this._pending
                    .get(oldtag)
                    ?.controller.abort(new Error(`9P request tag ${oldtag} flushed`));
                this._pending.delete(oldtag);
                const response = encodeP9Frame(MSG.RFLUSH, tag);
                this.endpoint.post(response);
                emitListeners(this._responseListeners, {
                    event: message.event,
                    request,
                    response,
                    target: this.target,
                });
                return;
            }

            const controller = new AbortController();
            this._pending.set(tag, { controller, request });
            emitListeners(this._requestListeners, {
                bytes: request,
                event: message.event,
                target: this.target,
            });

            const complete = (value) => {
                if (
                    controller.signal.aborted ||
                    this.closed ||
                    this._pending.get(tag)?.controller !== controller
                ) {
                    return;
                }
                this._pending.delete(tag);
                const response = cloneFrameBytes(value, "9P response frame");
                this.endpoint.post(response);

                emitListeners(this._responseListeners, {
                    event: message.event,
                    request,
                    response,
                    target: this.target,
                });
            };
            const fail = (error) => {
                if (
                    controller.signal.aborted ||
                    this.closed ||
                    this._pending.get(tag)?.controller !== controller
                ) {
                    return;
                }
                this._pending.delete(tag);
                try {
                    this.endpoint.post(encodeP9ErrorFrame(tag, ERROR_EIO));
                } catch (postError) {
                    emitListeners(this._errorListeners, postError);
                }
                emitListeners(this._errorListeners, error);
            };
            const result = this.facade.handle9pFrame(request, {
                signal: controller.signal,
                tag,
                type: frameType(request),
            });
            if (result && typeof result.then === "function") {
                result.then(complete, fail);
            } else {
                complete(result);
            }
        } catch (error) {
            if (request) {
                try {
                    this.endpoint.post(encodeP9ErrorFrame(frameTag(request), ERROR_EIO));
                } catch (postError) {
                    emitListeners(this._errorListeners, postError);
                }
            }
            emitListeners(this._errorListeners, error);
        }
    }
}

export class Star9P9FramePortClient {
    constructor(target, options = {}) {
        this.endpoint = new BinaryMessageEndpoint(target, {
            autoStart: false,
            transfer: options.transfer,
        });
        this.target = this.endpoint.target;
        this.closed = false;
        this.started = false;
        this._pending = new Map();
        this._notagPending = [];
        this._cancelledTags = new Set();
        this._responseListeners = new Set();
        this._errorListeners = new Set();

        this.endpoint.onMessage((message) => {
            this._handleMessage(message);
        });
        this.endpoint.onError((error) => {
            this._emitError(error);
        });

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

    onResponse(listener) {
        return addListener(this._responseListeners, listener, "9P frame client response listener");
    }

    onError(listener) {
        return addListener(this._errorListeners, listener, "9P frame client error listener");
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

    request(frame, options = {}) {
        if (this.closed) {
            return Promise.reject(new Error("9P frame client is closed"));
        }
        const request = cloneFrameBytes(frame, "9P request frame");
        const tag = frameTag(request);
        return new Promise((resolve, reject) => {
            const pending = {
                reject,
                request,
                resolve,
                signal: options.signal || null,
                tag,
                onAbort: null,
            };
            this._trackPending(tag, pending);
            if (pending.signal) {
                if (pending.signal.aborted) {
                    this._cancelPending(tag, pending, abortReason(pending.signal));
                    return;
                }
                pending.onAbort = () => {
                    this._cancelPending(tag, pending, abortReason(pending.signal));
                };
                pending.signal.addEventListener("abort", pending.onAbort, { once: true });
            }
            try {
                this.endpoint.post(request);
            } catch (error) {
                this._untrackPending(tag, pending);
                reject(error);
            }
        });
    }

    close() {
        if (this.closed) {
            return this;
        }
        this.stop();
        const error = new Error("9P frame client closed");
        for (const [tag, pending] of [...this._pending.entries()]) {
            this._untrackPending(tag, pending);
            pending.reject(error);
        }
        for (const pending of [...this._notagPending]) {
            this._untrackPending(0xffff, pending);
            pending.reject(error);
        }
        this._pending.clear();
        this._notagPending.length = 0;
        if (typeof this.target.close === "function") {
            this.target.close();
        }
        this.closed = true;
        return this;
    }

    _trackPending(tag, pending) {
        if (tag === 0xffff) {
            this._notagPending.push(pending);
            return;
        }
        if (this._pending.has(tag)) {
            throw new Error(`duplicate 9P request tag ${tag}`);
        }
        this._pending.set(tag, pending);
    }

    _untrackPending(tag, pending) {
        if (pending?.signal && pending.onAbort) {
            pending.signal.removeEventListener("abort", pending.onAbort);
            pending.onAbort = null;
        }
        if (tag === 0xffff) {
            const index = this._notagPending.indexOf(pending);
            if (index >= 0) {
                this._notagPending.splice(index, 1);
            }
            return;
        }
        if (this._pending.get(tag) === pending) {
            this._pending.delete(tag);
        }
    }

    _cancelPending(tag, pending, reason) {
        this._untrackPending(tag, pending);
        if (tag !== 0xffff) {
            this._cancelledTags.add(tag);
            try {
                this.flush(tag);
            } catch (error) {
                this._emitError(error);
            }
        }
        pending.reject(reason);
    }

    flush(oldtag) {
        if (this.closed) {
            throw new Error("9P frame client is closed");
        }
        const tag = this._allocSyntheticTag(oldtag);
        this._cancelledTags.add(tag);
        const writer = new P9Writer();
        writer.u16(oldtag);
        this.endpoint.post(encodeP9Frame(MSG.TFLUSH, tag, writer.finish()));
        return tag;
    }

    _handleMessage(message) {
        let response = null;
        try {
            response = cloneFrameBytes(message.bytes, "9P response frame");
            const tag = frameTag(response);
            const pending =
                tag === 0xffff ? this._notagPending.shift() : this._pending.get(tag);
            if (!pending) {
                if (this._cancelledTags.delete(tag)) {
                    return;
                }
                throw new Error(`received 9P response for unknown tag ${tag}`);
            }
            if (tag !== 0xffff) {
                this._pending.delete(tag);
            }
            if (pending.signal && pending.onAbort) {
                pending.signal.removeEventListener("abort", pending.onAbort);
                pending.onAbort = null;
            }
            pending.resolve(response);
            emitListeners(this._responseListeners, {
                event: message.event,
                request: pending.request,
                response,
                tag,
                target: this.target,
            });
        } catch (error) {
            this._emitError(error);
        }
    }

    _emitError(error) {
        emitListeners(this._errorListeners, error);
    }

    _allocSyntheticTag(avoidTag = null) {
        for (let tag = 1; tag < 0xffff; tag += 1) {
            if (
                tag !== avoidTag &&
                !this._pending.has(tag) &&
                !this._cancelledTags.has(tag)
            ) {
                return tag;
            }
        }
        throw new Error("no free 9P tag available");
    }
}

export class Star9P9NamespaceMount {
    static async connect(target, options = {}) {
        const mount = new Star9P9NamespaceMount(target, options);
        await mount.connect();
        return mount;
    }

    constructor(target, options = {}) {
        this.client =
            target instanceof Star9P9FramePortClient
                ? target
                : new Star9P9FramePortClient(target, options.client);
        this.closed = false;
        this.msize = DEFAULT_MSIZE;
        this.rootFid = 1;
        this._ownsClient = !(target instanceof Star9P9FramePortClient);
        this._nextFid = 2;
        this._nextTag = 1;
        this._textEncoder = options.textEncoder || new TextEncoder();
        this._textDecoder = options.textDecoder || new TextDecoder();
    }

    async connect() {
        const version = await this._call(MSG.TVERSION, (writer) => {
            writer.u32(DEFAULT_MSIZE);
            writer.string(VERSION);
        });
        if (version.type !== MSG.RVERSION) {
            throw new Error(`expected 9P Rversion, got message type ${version.type}`);
        }
        const versionReader = new P9Reader(version.body);
        this.msize = versionReader.u32();
        const negotiated = versionReader.string();
        versionReader.finish();
        if (negotiated !== VERSION) {
            throw new Error(`unsupported 9P version ${JSON.stringify(negotiated)}`);
        }

        const attach = await this._call(MSG.TATTACH, (writer) => {
            writer.u32(this.rootFid);
            writer.u32(NOFID);
            writer.string("star9");
            writer.string("");
            writer.u32(0);
        });
        this._expect(attach, MSG.RATTACH);
        return this;
    }

    async stat(path = ".") {
        const cleanPath = normalizeP9Path(path);
        const fid = await this._walk(cleanPath);
        try {
            const response = await this._call(MSG.TGETATTR, (writer) => {
                writer.u32(fid);
                writer.u64(ATTR_BASIC);
            });
            this._expect(response, MSG.RGETATTR);
            return metadataFromAttr(cleanPath, new P9Reader(response.body).attr());
        } finally {
            await this._clunk(fid);
        }
    }

    async readFile(path) {
        const cleanPath = normalizeP9Path(path);
        const fid = await this._walk(cleanPath);
        try {
            await this._lopen(fid, OPEN_RDONLY);
            const chunks = [];
            let offset = 0;
            const chunkSize = Math.max(1, Math.min(32 * 1024, this.msize - 11));
            while (true) {
                const response = await this._call(MSG.TREAD, (writer) => {
                    writer.u32(fid);
                    writer.u64(offset);
                    writer.u32(chunkSize);
                });
                this._expect(response, MSG.RREAD);
                const data = new P9Reader(response.body).countedData();
                if (data.byteLength === 0) {
                    return concatBytes(chunks);
                }
                chunks.push(data);
                offset += data.byteLength;
            }
        } finally {
            await this._clunk(fid);
        }
    }

    async readText(path) {
        return this._textDecoder.decode(await this.readFile(path));
    }

    async writeFile(path, bytes) {
        const cleanPath = normalizeP9Path(path);
        if (cleanPath === ".") {
            throw new Error("cannot write remote 9P root as a file");
        }
        const data = toUint8Array(bytes, "9P writeFile bytes").slice();
        const parent = parentPath(cleanPath);
        const name = baseName(cleanPath);
        const fid = await this._walk(parent);
        try {
            const create = await this._call(MSG.TLCREATE, (writer) => {
                writer.u32(fid);
                writer.string(name);
                writer.u32(OPEN_RDWR | OPEN_TRUNC);
                writer.u32(0o644);
                writer.u32(0);
            });
            this._expect(create, MSG.RLCREATE);
            await this._writeAll(fid, data);
        } finally {
            await this._clunk(fid);
        }
    }

    async writeText(path, text) {
        await this.writeFile(path, this._textEncoder.encode(String(text)));
    }

    async readDir(path = ".") {
        const cleanPath = normalizeP9Path(path);
        const fid = await this._walk(cleanPath);
        try {
            await this._lopen(fid, OPEN_RDONLY);
            const entries = [];
            let offset = 0;
            while (true) {
                const response = await this._call(MSG.TREADDIR, (writer) => {
                    writer.u32(fid);
                    writer.u64(offset);
                    writer.u32(Math.max(1, Math.min(32 * 1024, this.msize - 11)));
                });
                this._expect(response, MSG.RREADDIR);
                const chunk = new P9Reader(response.body).countedData();
                if (chunk.byteLength === 0) {
                    return entries;
                }
                const decoded = decodeDirents(chunk);
                if (decoded.length === 0) {
                    return entries;
                }
                entries.push(...decoded.map((entry) => ({
                    name: entry.name,
                    path: joinPath(cleanPath, entry.name),
                    kind: entry.type === DT_DIR ? "dir" : entry.type === DT_LNK ? "symlink" : "file",
                    type: entry.type === DT_DIR ? "dir" : entry.type === DT_LNK ? "symlink" : "file",
                    size: 0,
                })));
                offset = decoded[decoded.length - 1].offset;
            }
        } finally {
            await this._clunk(fid);
        }
    }

    async mkdir(path) {
        const cleanPath = normalizeP9Path(path);
        if (cleanPath === ".") {
            throw new Error("remote 9P root already exists");
        }
        const parent = await this._walk(parentPath(cleanPath));
        try {
            const response = await this._call(MSG.TMKDIR, (writer) => {
                writer.u32(parent);
                writer.string(baseName(cleanPath));
                writer.u32(0o755);
                writer.u32(0);
            });
            this._expect(response, MSG.RMKDIR);
        } finally {
            await this._clunk(parent);
        }
    }

    async remove(path) {
        const cleanPath = normalizeP9Path(path);
        if (cleanPath === ".") {
            throw new Error("refusing to remove remote 9P root");
        }
        let isDir = false;
        try {
            isDir = (await this.stat(cleanPath)).kind === "dir";
        } catch {
            isDir = false;
        }
        const parent = await this._walk(parentPath(cleanPath));
        try {
            const response = await this._call(MSG.TUNLINKAT, (writer) => {
                writer.u32(parent);
                writer.string(baseName(cleanPath));
                writer.u32(isDir ? AT_REMOVEDIR : 0);
            });
            this._expect(response, MSG.RUNLINKAT);
        } finally {
            await this._clunk(parent);
        }
    }

    close() {
        this.closed = true;
        if (this._ownsClient) {
            this.client.close();
        }
    }

    async _lopen(fid, flags) {
        const response = await this._call(MSG.TLOPEN, (writer) => {
            writer.u32(fid);
            writer.u32(flags);
        });
        this._expect(response, MSG.RLOPEN);
    }

    async _writeAll(fid, data) {
        const chunkSize = Math.max(1, Math.min(32 * 1024, this.msize - 23));
        let offset = 0;
        while (offset < data.byteLength) {
            const chunk = data.slice(offset, offset + chunkSize);
            const response = await this._call(MSG.TWRITE, (writer) => {
                writer.u32(fid);
                writer.u64(offset);
                writer.countedData(chunk);
            });
            this._expect(response, MSG.RWRITE);
            const count = new P9Reader(response.body).u32();
            if (count !== chunk.byteLength) {
                throw new Error(`short 9P write: wrote ${count} of ${chunk.byteLength}`);
            }
            offset += count;
        }
    }

    async _walk(path) {
        const fid = this._allocFid();
        const names = path === "." ? [] : path.split("/");
        const response = await this._call(MSG.TWALK, (writer) => {
            writer.u32(this.rootFid);
            writer.u32(fid);
            writer.u16(names.length);
            for (const name of names) {
                writer.string(name);
            }
        });
        this._expect(response, MSG.RWALK);
        const reader = new P9Reader(response.body);
        const qidCount = reader.u16();
        for (let index = 0; index < qidCount; index += 1) {
            reader.qid();
        }
        reader.finish();
        if (qidCount !== names.length) {
            await this._clunk(fid).catch(() => {});
            throw new Error(`remote 9P path not found: ${path}`);
        }
        return fid;
    }

    async _clunk(fid) {
        const response = await this._call(MSG.TCLUNK, (writer) => {
            writer.u32(fid);
        });
        this._expect(response, MSG.RCLUNK);
    }

    async _call(type, buildBody) {
        if (this.closed) {
            throw new Error("9P namespace mount is closed");
        }
        const tag = this._allocTag();
        const writer = new P9Writer();
        buildBody?.(writer);
        const frame = encodeP9Frame(type, tag, writer.finish());
        const responseFrame = await this.client.request(frame);
        const response = decodeP9Frame(responseFrame);
        if (response.tag !== tag) {
            throw new Error(`9P tag mismatch: expected ${tag}, got ${response.tag}`);
        }
        if (response.type === MSG.RLERROR) {
            const ecode = new P9Reader(response.body).u32();
            throw new Error(`remote 9P error ${ecode}`);
        }
        return response;
    }

    _expect(response, type) {
        if (response.type !== type) {
            throw new Error(`expected 9P message type ${type}, got ${response.type}`);
        }
    }

    _allocFid() {
        return this._nextFid++;
    }

    _allocTag() {
        const tag = this._nextTag;
        this._nextTag = this._nextTag === 0xfffe ? 1 : this._nextTag + 1;
        return tag;
    }
}

export class Star9ImportResponder {
    constructor(target, facade, options = {}) {
        this.target = requireListenerTarget(target);
        this.facade = requireStar9P9Facade(facade);
        this.element = options.element || null;
        this.request = String(options.request || DEFAULT_STAR9_IMPORT_REQUEST);
        this.allowOrigins = normalizeAllowOrigins(options.allowOrigins);
        this.systemId = options.systemId || options.id || null;
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
        return addListener(this._requestListeners, listener, "star9 import request listener");
    }

    onError(listener) {
        return addListener(this._errorListeners, listener, "star9 import error listener");
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
        if (!originAllowed(event?.origin, this.allowOrigins)) {
            return;
        }
        if (this.systemId && typeof location !== "undefined" && location.hash.slice(1) !== this.systemId) {
            return;
        }

        let server = null;
        try {
            requireMessageChannel();
            const responder = requireMessagePort(payload.responder);
            startMessageTarget(responder);

            const channel = new MessageChannel();
            server = new Star9P9FramePortServer(channel.port1, this.facade, {
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

const VERSION = "9P2000.L";
const DEFAULT_MSIZE = 64 * 1024;
const NOFID = 0xffffffff;
const AT_REMOVEDIR = 0x200;
const ATTR_BASIC = 0x67f;
const ERROR_EIO = 5;
const OPEN_RDONLY = 0;
const OPEN_RDWR = 2;
const OPEN_TRUNC = 0o1000;
const DT_DIR = 4;
const DT_LNK = 10;

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
    TMKDIR: 72,
    RMKDIR: 73,
    TUNLINKAT: 76,
    RUNLINKAT: 77,
});

class P9Writer {
    constructor() {
        this.bytes = [];
    }

    u8(value) {
        this.bytes.push(value & 0xff);
    }

    u16(value) {
        this.bytes.push(value & 0xff, (value >> 8) & 0xff);
    }

    u32(value) {
        this.bytes.push(
            value & 0xff,
            (value >> 8) & 0xff,
            (value >> 16) & 0xff,
            (value >> 24) & 0xff,
        );
    }

    u64(value) {
        let bigint = BigInt(value);
        for (let index = 0; index < 8; index += 1) {
            this.bytes.push(Number((bigint >> BigInt(index * 8)) & 0xffn));
        }
    }

    string(value) {
        const bytes = new TextEncoder().encode(String(value));
        if (bytes.byteLength > 0xffff) {
            throw new Error("9P string is too long");
        }
        this.u16(bytes.byteLength);
        this.bytes.push(...bytes);
    }

    countedData(value) {
        const bytes = toUint8Array(value, "9P counted data");
        this.u32(bytes.byteLength);
        this.bytes.push(...bytes);
    }

    finish() {
        return Uint8Array.from(this.bytes);
    }
}

class P9Reader {
    constructor(bytes) {
        this.bytes = toUint8Array(bytes, "9P response body");
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
        const value = new TextDecoder().decode(this.bytes.slice(this.offset, this.offset + length));
        this.offset += length;
        return value;
    }

    countedData() {
        const length = this.u32();
        this._require(length);
        const value = this.bytes.slice(this.offset, this.offset + length);
        this.offset += length;
        this.finish();
        return value;
    }

    qid() {
        return {
            type: this.u8(),
            version: this.u32(),
            path: this.u64(),
        };
    }

    attr() {
        const attr = {
            valid: this.u64(),
            qid: this.qid(),
            mode: this.u32(),
            uid: this.u32(),
            gid: this.u32(),
            nlink: this.u64(),
            rdev: this.u64(),
            size: this.u64(),
            blksize: this.u64(),
            blocks: this.u64(),
            atimeSeconds: this.u64(),
            atimeNanoseconds: this.u64(),
            mtimeSeconds: this.u64(),
            mtimeNanoseconds: this.u64(),
            ctimeSeconds: this.u64(),
            ctimeNanoseconds: this.u64(),
            btimeSeconds: this.u64(),
            btimeNanoseconds: this.u64(),
            gen: this.u64(),
            dataVersion: this.u64(),
        };
        this.finish();
        return attr;
    }

    finish() {
        if (this.offset !== this.bytes.byteLength) {
            throw new Error("trailing bytes in 9P message body");
        }
    }

    _require(length) {
        if (this.offset + length > this.bytes.byteLength) {
            throw new Error("truncated 9P message body");
        }
    }
}

function encodeP9Frame(type, tag, body = new Uint8Array()) {
    const payload = toUint8Array(body, "9P frame body");
    const frame = new Uint8Array(7 + payload.byteLength);
    const size = frame.byteLength;
    frame[0] = size & 0xff;
    frame[1] = (size >> 8) & 0xff;
    frame[2] = (size >> 16) & 0xff;
    frame[3] = (size >> 24) & 0xff;
    frame[4] = type & 0xff;
    frame[5] = tag & 0xff;
    frame[6] = (tag >> 8) & 0xff;
    frame.set(payload, 7);
    return frame;
}

function decodeP9Frame(frame) {
    const bytes = cloneFrameBytes(frame, "9P response frame");
    return {
        type: bytes[4],
        tag: frameTag(bytes),
        body: bytes.slice(7),
    };
}

function decodeDirents(data) {
    const reader = new P9Reader(data);
    const entries = [];
    while (reader.offset < reader.bytes.byteLength) {
        entries.push({
            qid: reader.qid(),
            offset: reader.u64(),
            type: reader.u8(),
            name: reader.string(),
        });
    }
    return entries;
}

function metadataFromAttr(path, attr) {
    const kind = (attr.mode & 0o170000) === 0o040000
        ? "dir"
        : (attr.mode & 0o170000) === 0o120000
            ? "symlink"
            : "file";
    return {
        name: baseName(path),
        path,
        kind,
        type: kind,
        size: attr.size,
        mode: attr.mode,
        uid: attr.uid,
        gid: attr.gid,
        modifiedMs: attr.mtimeSeconds * 1000 + Math.floor(attr.mtimeNanoseconds / 1_000_000),
    };
}

function normalizeP9Path(path) {
    if (path == null || path === "" || path === ".") {
        return ".";
    }
    const value = String(path);
    if (value.startsWith("/") || value.includes("\\")) {
        throw new Error(`invalid Star9 path ${JSON.stringify(path)}`);
    }
    const parts = [];
    for (const part of value.split("/")) {
        if (!part || part === ".") {
            continue;
        }
        if (part === "..") {
            throw new Error(`path traversal is not allowed: ${JSON.stringify(path)}`);
        }
        parts.push(part);
    }
    return parts.length === 0 ? "." : parts.join("/");
}

function baseName(path) {
    const cleanPath = normalizeP9Path(path);
    if (cleanPath === ".") {
        return ".";
    }
    return cleanPath.slice(cleanPath.lastIndexOf("/") + 1);
}

function parentPath(path) {
    const cleanPath = normalizeP9Path(path);
    if (cleanPath === "." || !cleanPath.includes("/")) {
        return ".";
    }
    return cleanPath.slice(0, cleanPath.lastIndexOf("/"));
}

function joinPath(parent, child) {
    const cleanParent = normalizeP9Path(parent);
    const cleanChild = normalizeP9Path(child);
    if (cleanParent === ".") {
        return cleanChild;
    }
    if (cleanChild === ".") {
        return cleanParent;
    }
    return `${cleanParent}/${cleanChild}`;
}

function concatBytes(chunks) {
    const total = chunks.reduce((sum, chunk) => sum + chunk.byteLength, 0);
    const out = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
        out.set(chunk, offset);
        offset += chunk.byteLength;
    }
    return out;
}

function normalizeAllowOrigins(value) {
    if (value == null) {
        return null;
    }
    if (Array.isArray(value)) {
        return value.map(String).filter(Boolean);
    }
    return String(value).split(/\s+/).filter(Boolean);
}

function originAllowed(origin, allowOrigins) {
    if (!allowOrigins || allowOrigins.length === 0) {
        return true;
    }
    return allowOrigins.includes("*") || allowOrigins.includes(origin);
}

function readElementFacade(element) {
    try {
        return element?.system || null;
    } catch {
        return null;
    }
}

function requireStar9P9Facade(facade) {
    if (!isStar9P9FacadeLike(facade)) {
        throw new TypeError("expected a Star9System facade with handle9pFrame(frame)");
    }
    return facade;
}

function cloneFrameBytes(value, label) {
    const frame = toUint8Array(value, label).slice();
    validateFrameShape(frame, label);
    return frame;
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

function frameTag(frame) {
    validateFrameShape(frame, "9P frame");
    return frame[5] | (frame[6] << 8);
}

function frameType(frame) {
    validateFrameShape(frame, "9P frame");
    return frame[4];
}

function flushOldTag(frame) {
    validateFrameShape(frame, "9P Tflush frame");
    if (frame.byteLength < 9) {
        throw new TypeError("expected 9P Tflush frame to include oldtag");
    }
    return frame[7] | (frame[8] << 8);
}

function validateFrameShape(frame, label) {
    if (frame.byteLength < 7) {
        throw new TypeError(`expected ${label} to be a complete 9P frame`);
    }
    const declared =
        frame[0] | (frame[1] << 8) | (frame[2] << 16) | (frame[3] << 24);
    if (declared !== frame.byteLength) {
        throw new TypeError(`expected ${label} length prefix to match frame size`);
    }
}

function encodeP9ErrorFrame(tag, ecode) {
    const writer = new P9Writer();
    writer.u32(ecode);
    return encodeP9Frame(MSG.RLERROR, tag, writer.finish());
}

function abortReason(signal) {
    if (signal && "reason" in signal && signal.reason !== undefined) {
        return signal.reason instanceof Error ? signal.reason : new Error(String(signal.reason));
    }
    return new Error("9P request aborted");
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

function isStar9P9FacadeLike(value) {
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
