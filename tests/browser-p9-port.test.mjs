import test from "node:test";
import assert from "node:assert/strict";

import {
    attachWanixImportResponder,
    createWanixP9FramePort,
    serveWanixP9FramePort,
} from "../crates/wanix-web/js/p9-port.js";

test("serveWanixP9FramePort handles binary request frames and posts binary responses", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    const requests = [];
    const facade = {
        handle9pFrame(frame) {
            requests.push(frame);
            return new Uint8Array([4, 5, 6, frame[0] ?? 0]);
        },
    };

    const server = await serveWanixP9FramePort(channel.port1, facade);
    t.after(() => server.close());

    const responses = [];
    channel.port2.addEventListener("message", (event) => {
        responses.push(event.data);
    });
    channel.port2.start();

    const request = new Uint8Array([1, 2, 3, 4]);
    channel.port2.postMessage(request);

    assert.equal(requests.length, 1);
    assert.deepEqual(requests[0], request);
    assert.equal(responses.length, 1);
    assert.deepEqual(responses[0], new Uint8Array([4, 5, 6, 1]));
});

test("serveWanixP9FramePort reports non-binary request frames through error listeners", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const channel = new MessageChannel();
    let calls = 0;
    const errors = [];
    const server = await serveWanixP9FramePort(channel.port1, {
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

test("attachWanixImportResponder transfers a served MessagePort-like 9P endpoint", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const target = new FakeWindowTarget();
    const frames = [];
    const servedServers = [];
    const responder = await attachWanixImportResponder(target, {
        handle9pFrame(frame) {
            frames.push(frame);
            return new Uint8Array([7, 7, frame.byteLength]);
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
        request: "wanix-import",
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
    port.postMessage(new Uint8Array([8, 9, 10, 11]));

    assert.equal(frames.length, 1);
    assert.deepEqual(frames[0], new Uint8Array([8, 9, 10, 11]));
    assert.equal(responses.length, 1);
    assert.deepEqual(responses[0], new Uint8Array([7, 7, 4]));
    assert.equal(servedServers.length, 1);
    responder.close();
    assert.equal(servedServers[0].closed, true);
});

test("createWanixP9FramePort creates a transferable served endpoint", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const created = await createWanixP9FramePort({
        handle9pFrame(frame) {
            return new Uint8Array([frame[0] ?? 0, 42]);
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
    created.port.postMessage(new Uint8Array([3]));

    assert.equal(created.server.started, true);
    assert.equal(created.server.closed, false);
    assert.equal(responses.length, 1);
    assert.deepEqual(responses[0], new Uint8Array([3, 42]));
});

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

    dispatchMessage(data) {
        for (const listener of this._listeners.get("message") ?? []) {
            listener({ data });
        }
    }
}
