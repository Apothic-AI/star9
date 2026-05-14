import test from "node:test";
import assert from "node:assert/strict";

import {
    attachStar9ImportResponder,
    createStar9P9FramePort,
    createStar9P9FrameClient,
    createStar9P9NamespaceMount,
    serveStar9P9FramePort,
} from "../crates/star9-web/js/p9-port.js";
import { createStorageP9FramePort } from "../crates/star9-web/js/storage-p9.js";

test("serveStar9P9FramePort handles binary request frames and posts binary responses", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    const requests = [];
    const facade = {
        handle9pFrame(frame) {
            requests.push(frame);
            return p9Frame(101, tagOf(frame), [frame[4]]);
        },
    };

    const server = await serveStar9P9FramePort(channel.port1, facade);
    t.after(() => server.close());

    const responses = [];
    channel.port2.addEventListener("message", (event) => {
        responses.push(event.data);
    });
    channel.port2.start();

    const request = p9Frame(100, 7, [9]);
    channel.port2.postMessage(request);

    assert.equal(requests.length, 1);
    assert.deepEqual(requests[0], request);
    assert.equal(responses.length, 1);
    assert.deepEqual(responses[0], p9Frame(101, 7, [100]));
});

test("serveStar9P9FramePort reports non-binary request frames through error listeners", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    let calls = 0;
    const errors = [];
    const server = await serveStar9P9FramePort(channel.port1, {
        handle9pFrame() {
            calls += 1;
            return new Uint8Array([9]);
        },
    });
    t.after(() => server.close());

    server.onError((error) => {
        errors.push(error);
    });

    channel.port2.start();
    channel.port2.postMessage({ invalid: true });

    assert.equal(calls, 0);
    assert.equal(errors.length, 1);
    assert.match(String(errors[0]), /expected binary runtime message to be binary data/);
});

test("serveStar9P9FramePort returns Rlerror when the facade throws after a valid request", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    const errors = [];
    const server = await serveStar9P9FramePort(channel.port1, {
        handle9pFrame() {
            throw new Error("boom");
        },
    });
    server.onError((error) => errors.push(error));
    t.after(() => server.close());

    const responses = [];
    channel.port2.addEventListener("message", (event) => {
        responses.push(event.data);
    });
    channel.port2.start();
    channel.port2.postMessage(p9Frame(100, 77));

    assert.equal(errors.length, 1);
    assert.match(String(errors[0]), /boom/);
    assert.equal(responses.length, 1);
    assert.deepEqual(responses[0], p9Frame(7, 77, [5, 0, 0, 0]));
});

test("attachStar9ImportResponder transfers a served MessagePort-like 9P endpoint", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const target = new FakeWindowTarget();
    const frames = [];
    const servedServers = [];
    const responder = await attachStar9ImportResponder(target, {
        handle9pFrame(frame) {
            frames.push(frame);
            return p9Frame(111, tagOf(frame), [tagOf(frame)]);
        },
    });
    responder.onRequest(({ server }) => {
        servedServers.push(server);
    });
    t.after(() => responder.close());

    const replyChannel = new MessageChannel();
    const transferred = [];
    replyChannel.port1.addEventListener("message", (event) => {
        transferred.push(event.data);
    });
    replyChannel.port1.start();

    target.dispatchMessage({
        request: "star9-import",
        responder: replyChannel.port2,
    });

    assert.equal(transferred.length, 1);
    const port = transferred[0];
    assert.equal(typeof port.postMessage, "function");
    assert.equal(typeof port.start, "function");
    assert.equal(typeof port.close, "function");

    const responses = [];
    port.addEventListener("message", (event) => {
        responses.push(event.data);
    });
    port.start();
    port.postMessage(p9Frame(110, 9, [1, 2]));

    assert.equal(frames.length, 1);
    assert.deepEqual(frames[0], p9Frame(110, 9, [1, 2]));
    assert.equal(responses.length, 1);
    assert.deepEqual(responses[0], p9Frame(111, 9, [9]));
    assert.equal(servedServers.length, 1);
    responder.close();
    assert.equal(servedServers[0].closed, true);
});

test("createStar9P9FramePort creates a transferable served endpoint", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const created = await createStar9P9FramePort({
        handle9pFrame(frame) {
            return p9Frame(121, tagOf(frame), [42]);
        },
    });
    t.after(() => {
        created.server.close();
        created.port.close();
    });

    const responses = [];
    created.port.addEventListener("message", (event) => {
        responses.push(event.data);
    });
    created.port.start();
    created.port.postMessage(p9Frame(120, 11));

    assert.equal(created.server.started, true);
    assert.equal(created.server.closed, false);
    assert.equal(responses.length, 1);
    assert.deepEqual(responses[0], p9Frame(121, 11, [42]));
});

test("Star9P9FramePortClient resolves responses by 9P tag", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    const server = await serveStar9P9FramePort(channel.port1, {
        handle9pFrame(frame) {
            return p9Frame(frame[4] + 1, tagOf(frame), [frame.at(-1)]);
        },
    });
    t.after(() => server.close());

    const client = createStar9P9FrameClient(channel.port2);
    t.after(() => client.close());

    const first = await client.request(p9Frame(30, 21, [1]));
    const second = await client.request(p9Frame(32, 22, [2]));

    assert.deepEqual(first, p9Frame(31, 21, [1]));
    assert.deepEqual(second, p9Frame(33, 22, [2]));
});

test("Star9P9FramePortClient reports unknown response tags", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    const errors = [];
    const client = createStar9P9FrameClient(channel.port1);
    client.onError((error) => errors.push(error));
    t.after(() => client.close());

    channel.port2.start();
    channel.port2.postMessage(p9Frame(41, 99));

    assert.equal(errors.length, 1);
    assert.match(String(errors[0]), /unknown tag 99/);
});

test("Star9P9FramePortClient aborts requests with Tflush and ignores late responses", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    const sent = [];
    channel.port1.addEventListener("message", (event) => {
        sent.push(event.data);
    });
    channel.port1.start();

    const errors = [];
    const client = createStar9P9FrameClient(channel.port2);
    client.onError((error) => errors.push(error));
    t.after(() => client.close());

    const controller = new AbortController();
    const pending = client.request(p9Frame(116, 42, [1, 2, 3, 4]), {
        signal: controller.signal,
    });

    assert.equal(sent.length, 1);
    assert.deepEqual(sent[0], p9Frame(116, 42, [1, 2, 3, 4]));

    controller.abort(new Error("cancelled read"));
    await assert.rejects(pending, /cancelled read/);

    assert.equal(sent.length, 2);
    const flush = sent[1];
    assert.equal(flush[4], 108);
    assert.notEqual(tagOf(flush), 42);
    assert.equal(flush[7] | (flush[8] << 8), 42);

    channel.port1.postMessage(p9Frame(117, 42, [0, 0, 0, 0]));
    channel.port1.postMessage(p9Frame(109, tagOf(flush)));
    assert.deepEqual(errors, []);
});

test("Star9P9FramePortServer aborts async work on Tflush and suppresses late replies", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    let aborted = false;
    let resolveRead = null;
    const serverResponses = [];
    const server = await serveStar9P9FramePort(channel.port1, {
        handle9pFrame(frame, context) {
            context.signal.addEventListener("abort", () => {
                aborted = true;
                resolveRead(p9Frame(117, tagOf(frame), [1, 2, 3, 4]));
            });
            return new Promise((resolve) => {
                resolveRead = resolve;
            });
        },
    });
    server.onResponse(({ response }) => serverResponses.push(response));
    t.after(() => server.close());

    const client = createStar9P9FrameClient(channel.port2);
    t.after(() => client.close());

    const controller = new AbortController();
    const pending = client.request(p9Frame(116, 44, [0, 0, 0, 0]), {
        signal: controller.signal,
    });

    controller.abort(new Error("server-side cancel"));
    await assert.rejects(pending, /server-side cancel/);
    await flushTasks();

    assert.equal(aborted, true);
    assert.equal(serverResponses.length, 1);
    assert.equal(serverResponses[0][4], 109);
    assert.notEqual(tagOf(serverResponses[0]), 44);
});

test("Star9P9NamespaceMount reads, writes, and lists over MessagePort 9P", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    const server = new FakeNinePServer(channel.port1, {
        "hello.txt": encoder.encode("hello"),
    });
    t.after(() => server.close());

    const mount = await createStar9P9NamespaceMount(channel.port2);
    t.after(() => mount.close());

    assert.equal(await mount.readText("hello.txt"), "hello");
    await mount.writeText("created.txt", "created");
    assert.equal(await mount.readText("created.txt"), "created");
    assert.deepEqual(
        (await mount.readDir(".")).map((entry) => entry.name),
        ["created.txt", "hello.txt"],
    );
});

test("createStorageP9FramePort exports async storage adapters as 9P mounts", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const adapter = new MemoryStorageAdapter({
        "hello.txt": encoder.encode("hello"),
    });
    const exported = createStorageP9FramePort(adapter);
    t.after(() => {
        exported.server.close();
        exported.port.close();
    });

    const mount = await createStar9P9NamespaceMount(exported.port);
    t.after(() => mount.close());

    assert.equal(await mount.readText("hello.txt"), "hello");
    await mount.mkdir("docs");
    await mount.writeText("docs/created.txt", "created");
    assert.equal(await mount.readText("docs/created.txt"), "created");
    assert.deepEqual(
        (await mount.readDir("docs")).map((entry) => entry.name),
        ["created.txt"],
    );
    await mount.remove("docs/created.txt");
    await assert.rejects(
        () => mount.readText("docs/created.txt"),
        /remote 9P (?:error 2|path not found)/,
    );
});

test("storage 9P exports large writes and complete directory chunks", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const adapter = new MemoryStorageAdapter();
    const exported = createStorageP9FramePort(adapter, { msize: 192 });
    t.after(() => {
        exported.server.close();
        exported.port.close();
    });

    const mount = await createStar9P9NamespaceMount(exported.port);
    t.after(() => mount.close());

    const payload = "x".repeat(130_000);
    await mount.writeText("large.txt", payload);
    assert.equal(await mount.readText("large.txt"), payload);

    await mount.mkdir("many");
    for (let index = 0; index < 48; index += 1) {
        await mount.writeText(`many/entry-${String(index).padStart(2, "0")}-long-name.txt`, String(index));
    }
    const names = (await mount.readDir("many")).map((entry) => entry.name);
    assert.equal(names.length, 48);
    assert.equal(names[0], "entry-00-long-name.txt");
    assert.equal(names.at(-1), "entry-47-long-name.txt");
});

test("storage 9P server aborts pending adapter work on Tflush and rejects malformed frames", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const adapter = new SlowReadStorageAdapter({
        "slow.txt": encoder.encode("slow-data"),
    });
    const exported = createStorageP9FramePort(adapter);
    t.after(() => {
        exported.server.close();
        exported.port.close();
    });

    const errors = [];
    const client = createStar9P9FrameClient(exported.port);
    client.onError((error) => errors.push(error));
    t.after(() => client.close());

    await client.request(requestFrame(100, 1, (out) => {
        out.u32(4096);
        out.string("9P2000.L");
    }));
    await client.request(requestFrame(104, 2, (out) => {
        out.u32(1);
        out.u32(0xffffffff);
        out.string("star9");
        out.string("");
        out.u32(0);
    }));
    await client.request(requestFrame(110, 3, (out) => {
        out.u32(1);
        out.u32(2);
        out.u16(1);
        out.string("slow.txt");
    }));

    const controller = new AbortController();
    const pending = client.request(requestFrame(116, 4, (out) => {
        out.u32(2);
        out.u64(0);
        out.u32(8);
    }), {
        signal: controller.signal,
    });
    await adapter.waitForRead();
    assert.equal(adapter.lastSignal.aborted, false);

    controller.abort(new Error("cancel storage read"));
    await assert.rejects(pending, /cancel storage read/);
    assert.equal(adapter.lastSignal.aborted, true);
    adapter.releaseRead();
    await flushTasks();
    assert.deepEqual(errors, []);

    const responses = [];
    exported.port.addEventListener("message", (event) => responses.push(event.data));
    exported.port.start();
    const malformed = p9Frame(100, 99);
    malformed[0] = 99;
    exported.port.postMessage(malformed);
    assert.equal(responses.length, 1);
    assert.equal(responses[0][4], 7);
    assert.equal(tagOf(responses[0]), 0xffff);

    exported.port.postMessage("not-binary");
    assert.equal(responses.length, 2);
    assert.equal(responses[1][4], 7);
    assert.equal(tagOf(responses[1]), 0xffff);
});

test("star9 import responder enforces origins and supports concurrent imported mounts", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const target = new FakeWindowTarget();
    const servedServers = [];
    const responder = await attachStar9ImportResponder(target, {
        handle9pFrame(frame) {
            return p9Frame(frame[4] + 1, tagOf(frame), [tagOf(frame) & 0xff]);
        },
    }, {
        allowOrigins: ["https://allowed.example"],
        onrequest({ server }) {
            servedServers.push(server);
        },
    });
    t.after(() => responder.close());

    const denied = new MessageChannel();
    const deniedTransfers = [];
    denied.port1.addEventListener("message", (event) => deniedTransfers.push(event.data));
    denied.port1.start();
    target.dispatchMessage({
        request: "star9-import",
        responder: denied.port2,
    }, {
        origin: "https://denied.example",
    });
    assert.equal(deniedTransfers.length, 0);

    const [first, second] = [new MessageChannel(), new MessageChannel()];
    const imported = [];
    for (const channel of [first, second]) {
        channel.port1.addEventListener("message", (event) => imported.push(event.data));
        channel.port1.start();
        target.dispatchMessage({
            request: "star9-import",
            responder: channel.port2,
        }, {
            origin: "https://allowed.example",
        });
    }

    assert.equal(imported.length, 2);
    const responses = [];
    for (const [index, port] of imported.entries()) {
        port.addEventListener("message", (event) => responses.push({ index, frame: event.data }));
        port.start();
        port.postMessage(p9Frame(110, 20 + index, [1, index]));
    }

    assert.equal(responses.length, 2);
    assert.deepEqual(responses.map(({ frame }) => frame[4]), [111, 111]);
    assert.deepEqual(responses.map(({ frame }) => tagOf(frame)), [20, 21]);
    responder.close();
    assert.equal(servedServers.length, 2);
    assert.equal(servedServers.every((server) => server.closed), true);
});

function p9Frame(type, tag, payload = []) {
    const frame = new Uint8Array(7 + payload.length);
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

function requestFrame(type, tag, buildBody) {
    const out = new Writer();
    buildBody?.(out);
    return p9Frame(type, tag, out.bytes());
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

class FakeNinePServer {
    constructor(port, files = {}) {
        this.port = port;
        this.files = new Map(Object.entries(files));
        this.fids = new Map();
        this.port.addEventListener("message", (event) => {
            this.port.postMessage(this.handle(event.data));
        });
        this.port.start();
    }

    close() {
        this.port.close();
    }

    handle(frame) {
        const request = new Reader(frame);
        const size = request.u32();
        assert.equal(size, frame.byteLength);
        const type = request.u8();
        const tag = request.u16();
        const out = new Writer();

        switch (type) {
        case 100:
            out.u32(request.u32());
            out.string(request.string());
            return frameFrom(101, tag, out.bytes());
        case 104: {
            const fid = request.u32();
            this.fids.set(fid, ".");
            return frameFrom(105, tag, qid());
        }
        case 110: {
            const fid = request.u32();
            const newfid = request.u32();
            const count = request.u16();
            let path = this.fids.get(fid) || ".";
            const qids = [];
            for (let index = 0; index < count; index += 1) {
                path = join(path, request.string());
                if (path !== "." && !this.files.has(path)) {
                    break;
                }
                qids.push(qid());
            }
            this.fids.set(newfid, path);
            out.u16(qids.length);
            for (const value of qids) out.raw(value);
            return frameFrom(111, tag, out.bytes());
        }
        case 12:
            request.u32();
            request.u32();
            out.raw(qid());
            out.u32(0);
            return frameFrom(13, tag, out.bytes());
        case 14: {
            const fid = request.u32();
            const name = request.string();
            request.u32();
            request.u32();
            request.u32();
            const path = join(this.fids.get(fid), name);
            this.files.set(path, new Uint8Array());
            this.fids.set(fid, path);
            out.raw(qid());
            out.u32(0);
            return frameFrom(15, tag, out.bytes());
        }
        case 116: {
            const fid = request.u32();
            const offset = request.u64();
            const count = request.u32();
            const data = this.files.get(this.fids.get(fid)) || new Uint8Array();
            out.counted(data.slice(offset, offset + count));
            return frameFrom(117, tag, out.bytes());
        }
        case 118: {
            const fid = request.u32();
            const offset = request.u64();
            const data = request.counted();
            const path = this.fids.get(fid);
            const current = this.files.get(path) || new Uint8Array();
            const next = new Uint8Array(Math.max(current.byteLength, offset + data.byteLength));
            next.set(current);
            next.set(data, offset);
            this.files.set(path, next);
            out.u32(data.byteLength);
            return frameFrom(119, tag, out.bytes());
        }
        case 40: {
            const fid = request.u32();
            const offset = request.u64();
            const count = request.u32();
            const dirents = [...this.files.keys()].sort().map((name, index) => {
                const entry = new Writer();
                entry.raw(qid());
                entry.u64(index + 1);
                entry.u8(8);
                entry.string(name);
                return entry.bytes();
            }).filter((_, index) => index + 1 > offset);
            let body = new Uint8Array();
            for (const entry of dirents) {
                if (body.byteLength + entry.byteLength > count) break;
                const next = new Uint8Array(body.byteLength + entry.byteLength);
                next.set(body);
                next.set(entry, body.byteLength);
                body = next;
            }
            out.counted(body);
            return frameFrom(41, tag, out.bytes());
        }
        case 120:
            this.fids.delete(request.u32());
            return frameFrom(121, tag);
        default:
            out.u32(38);
            return frameFrom(7, tag, out.bytes());
        }
    }
}

class MemoryStorageAdapter {
    constructor(files = {}) {
        this.files = new Map(Object.entries(files));
        this.dirs = new Set(["."]);
        for (const path of this.files.keys()) {
            let current = ".";
            for (const part of path.split("/").slice(0, -1)) {
                current = current === "." ? part : `${current}/${part}`;
                this.dirs.add(current);
            }
        }
    }

    async stat(path = ".") {
        const normalized = clean(path);
        if (this.dirs.has(normalized)) {
            return { name: base(normalized), kind: "dir", type: "dir", size: 0 };
        }
        const file = this.files.get(normalized);
        if (!file) {
            throw storageError("ENOENT", `Path does not exist: ${normalized}`);
        }
        return { name: base(normalized), kind: "file", type: "file", size: file.byteLength };
    }

    async readFile(path) {
        const normalized = clean(path);
        const file = this.files.get(normalized);
        if (!file) {
            throw storageError("ENOENT", `Path does not exist: ${normalized}`);
        }
        return file.slice();
    }

    async writeFile(path, bytes) {
        const normalized = clean(path);
        const parent = parentPath(normalized);
        if (!this.dirs.has(parent)) {
            throw storageError("ENOENT", `Parent does not exist: ${parent}`);
        }
        this.files.set(normalized, new Uint8Array(bytes));
    }

    async readText(path) {
        return decoder.decode(await this.readFile(path));
    }

    async writeText(path, text) {
        await this.writeFile(path, encoder.encode(String(text)));
    }

    async readDir(path = ".") {
        const normalized = clean(path);
        if (!this.dirs.has(normalized)) {
            throw storageError("ENOENT", `Directory does not exist: ${normalized}`);
        }
        const prefix = normalized === "." ? "" : `${normalized}/`;
        const names = new Map();
        for (const dir of this.dirs) {
            if (dir === normalized || !dir.startsWith(prefix)) continue;
            const rest = dir.slice(prefix.length);
            const [name, ...tail] = rest.split("/");
            if (name && tail.length === 0) names.set(name, "dir");
        }
        for (const file of this.files.keys()) {
            if (!file.startsWith(prefix)) continue;
            const rest = file.slice(prefix.length);
            const [name, ...tail] = rest.split("/");
            if (name && tail.length === 0) names.set(name, "file");
        }
        return [...names.entries()]
            .sort((left, right) => left[0].localeCompare(right[0]))
            .map(([name, kind]) => ({ name, kind, type: kind, size: 0 }));
    }

    async mkdir(path) {
        const normalized = clean(path);
        const parent = parentPath(normalized);
        if (!this.dirs.has(parent)) {
            throw storageError("ENOENT", `Parent does not exist: ${parent}`);
        }
        this.dirs.add(normalized);
    }

    async remove(path) {
        const normalized = clean(path);
        if (this.files.delete(normalized)) {
            return;
        }
        if (this.dirs.delete(normalized)) {
            return;
        }
        throw storageError("ENOENT", `Path does not exist: ${normalized}`);
    }
}

class SlowReadStorageAdapter extends MemoryStorageAdapter {
    constructor(files = {}) {
        super(files);
        this.lastSignal = null;
        this._releaseRead = null;
        this._readStarted = null;
        this._readStartedPromise = new Promise((resolve) => {
            this._readStarted = resolve;
        });
    }

    async readFile(path, options = {}) {
        this.lastSignal = options.signal || null;
        this._readStarted();
        await new Promise((resolve) => {
            this._releaseRead = resolve;
        });
        return super.readFile(path);
    }

    waitForRead() {
        return this._readStartedPromise;
    }

    releaseRead() {
        this._releaseRead?.();
    }
}

class Writer {
    constructor() {
        this._bytes = [];
    }
    u8(value) { this._bytes.push(value & 0xff); }
    u16(value) { this._bytes.push(value & 0xff, (value >> 8) & 0xff); }
    u32(value) {
        this._bytes.push(value & 0xff, (value >> 8) & 0xff, (value >> 16) & 0xff, (value >> 24) & 0xff);
    }
    u64(value) {
        let bigint = BigInt(value);
        for (let index = 0; index < 8; index += 1) {
            this._bytes.push(Number((bigint >> BigInt(index * 8)) & 0xffn));
        }
    }
    string(value) {
        const bytes = encoder.encode(String(value));
        this.u16(bytes.byteLength);
        this.raw(bytes);
    }
    counted(value) {
        this.u32(value.byteLength);
        this.raw(value);
    }
    raw(value) {
        this._bytes.push(...value);
    }
    bytes() {
        return Uint8Array.from(this._bytes);
    }
}

class Reader {
    constructor(bytes) {
        this.bytes = bytes;
        this.offset = 0;
    }
    u8() { return this.bytes[this.offset++]; }
    u16() {
        const value = this.bytes[this.offset] | (this.bytes[this.offset + 1] << 8);
        this.offset += 2;
        return value;
    }
    u32() {
        const value = this.bytes[this.offset] | (this.bytes[this.offset + 1] << 8) | (this.bytes[this.offset + 2] << 16) | (this.bytes[this.offset + 3] << 24);
        this.offset += 4;
        return value >>> 0;
    }
    u64() {
        let value = 0n;
        for (let index = 0; index < 8; index += 1) {
            value |= BigInt(this.bytes[this.offset + index]) << BigInt(index * 8);
        }
        this.offset += 8;
        return Number(value);
    }
    string() {
        const length = this.u16();
        const value = decoder.decode(this.bytes.slice(this.offset, this.offset + length));
        this.offset += length;
        return value;
    }
    counted() {
        const length = this.u32();
        const value = this.bytes.slice(this.offset, this.offset + length);
        this.offset += length;
        return value;
    }
}

function frameFrom(type, tag, body = new Uint8Array()) {
    return p9Frame(type, tag, body);
}

function qid() {
    const out = new Writer();
    out.u8(0);
    out.u32(0);
    out.u64(1);
    return out.bytes();
}

function join(parent = ".", name = ".") {
    return parent === "." ? name : `${parent}/${name}`;
}

function clean(path = ".") {
    const value = String(path || ".");
    const parts = [];
    for (const part of value.split("/")) {
        if (!part || part === ".") continue;
        if (part === "..") throw storageError("EINVAL", "path traversal");
        parts.push(part);
    }
    return parts.length === 0 ? "." : parts.join("/");
}

function base(path = ".") {
    const normalized = clean(path);
    return normalized === "." ? "." : normalized.slice(normalized.lastIndexOf("/") + 1);
}

function parentPath(path = ".") {
    const normalized = clean(path);
    if (normalized === "." || !normalized.includes("/")) {
        return ".";
    }
    return normalized.slice(0, normalized.lastIndexOf("/"));
}

function storageError(code, message) {
    const error = new Error(message);
    error.code = code;
    return error;
}

function tagOf(frame) {
    return frame[5] | (frame[6] << 8);
}

async function flushTasks() {
    await Promise.resolve();
    await Promise.resolve();
}

function installFakeMessageChannel() {
    const originalMessageChannel = globalThis.MessageChannel;
    globalThis.MessageChannel = FakeMessageChannel;
    return () => {
        if (originalMessageChannel === undefined) {
            delete globalThis.MessageChannel;
        } else {
            globalThis.MessageChannel = originalMessageChannel;
        }
    };
}

class FakeMessageChannel {
    constructor() {
        this.port1 = new FakeMessagePort();
        this.port2 = new FakeMessagePort();
        this.port1._entangle(this.port2);
        this.port2._entangle(this.port1);
    }
}

class FakeMessagePort {
    constructor() {
        this.closed = false;
        this.started = false;
        this._peer = null;
        this._listeners = new Map();
        this._pending = [];
    }

    addEventListener(type, listener) {
        if (!this._listeners.has(type)) {
            this._listeners.set(type, new Set());
        }
        this._listeners.get(type).add(listener);
    }

    removeEventListener(type, listener) {
        this._listeners.get(type)?.delete(listener);
    }

    postMessage(data) {
        if (this.closed) {
            throw new Error("cannot postMessage on a closed FakeMessagePort");
        }
        this._peer?._enqueue({ data });
    }

    start() {
        if (this.closed || this.started) {
            return;
        }
        this.started = true;
        while (this._pending.length > 0) {
            this._dispatch("message", this._pending.shift());
        }
    }

    close() {
        this.closed = true;
        this._pending.length = 0;
    }

    _entangle(peer) {
        this._peer = peer;
    }

    _enqueue(event) {
        if (this.closed) {
            return;
        }
        if (!this.started) {
            this._pending.push(event);
            return;
        }
        this._dispatch("message", event);
    }

    _dispatch(type, event) {
        for (const listener of this._listeners.get(type) ?? []) {
            listener(event);
        }
    }
}

class FakeWindowTarget {
    constructor() {
        this._listeners = new Map();
    }

    addEventListener(type, listener) {
        if (!this._listeners.has(type)) {
            this._listeners.set(type, new Set());
        }
        this._listeners.get(type).add(listener);
    }

    removeEventListener(type, listener) {
        this._listeners.get(type)?.delete(listener);
    }

    dispatchMessage(data, options = {}) {
        for (const listener of this._listeners.get("message") ?? []) {
            listener({ data, ...options });
        }
    }
}
