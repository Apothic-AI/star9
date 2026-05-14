import test from "node:test";
import assert from "node:assert/strict";

import {
    BrowserWorkerHost,
    acceptBrowserWorkerHost,
    spawnBrowserWorkerHost,
} from "../crates/wanix-web/js/worker-host.js";
import {
    DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
    createWanixRuntimeNamespaceClient,
    decodeWorkerRuntimeEnvelope,
    encodeWorkerRuntimeEnvelope,
} from "../crates/wanix-web/js/worker-runtime.js";

test("spawnBrowserWorkerHost connects a runtime port and bootstraps system context", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const worker = new FakeWorkerTarget();
    const systemFacade = createSystemFacade();
    const systemElement = {
        id: "system-under-test",
        ready: Promise.resolve(),
        system: systemFacade,
    };

    const host = await spawnBrowserWorkerHost(() => worker, {
        system: systemElement,
        taskId: "task-17",
        workerId: "worker-17",
        descriptor: "runtime",
    });
    t.after(() => host.close());

    assert.equal(host.started, true);
    assert.equal(host.target, worker);
    assert.deepEqual(host.descriptor, { port_id: "runtime", name: "runtime" });
    assert.equal(worker.posted.length, 1);

    const [{ message, transfer }] = worker.posted;
    assert.equal(message.type, DEFAULT_RUNTIME_PORT_MESSAGE_TYPE);
    assert.equal(message.task_id, "task-17");
    assert.equal(message.worker_id, "worker-17");
    assert.equal(message.system_id, "system-under-test");
    assert.deepEqual(message.descriptor, { port_id: "runtime", name: "runtime" });
    assert.equal(transfer.length, 1);
    assert.equal(typeof transfer[0].postMessage, "function");
    assert.equal(host.facade, systemFacade);
    assert.equal(host.element, systemElement);
});

test("acceptBrowserWorkerHost routes binary request and response envelopes over the runtime port", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const worker = new FakeWorkerTarget();
    const host = await acceptBrowserWorkerHost(worker, {
        descriptor: { port_id: "runtime", name: "runtime" },
    });
    t.after(() => host.close());

    const runtimePort = worker.posted[0].transfer[0];
    runtimePort.start();

    const remoteMessages = [];
    runtimePort.addEventListener("message", (event) => {
        remoteMessages.push(decodeWorkerRuntimeEnvelope(event.data));
    });

    const responses = [];
    host.onResponse((message) => {
        responses.push(message);
    });

    const requestPayload = new Uint8Array([9, 8, 7, 6]);
    const responsePayload = new Uint8Array([1, 3, 3, 7]);

    assert.equal(host.sendRequest(requestPayload), 5);
    assert.equal(remoteMessages.length, 1);
    assert.equal(remoteMessages[0].kind, "request");
    assert.deepEqual(remoteMessages[0].payload, requestPayload);

    runtimePort.postMessage(encodeWorkerRuntimeEnvelope("response", responsePayload));

    assert.equal(responses.length, 1);
    assert.equal(responses[0].kind, "response");
    assert.deepEqual(responses[0].payload, responsePayload);
});

test("spawnBrowserWorkerHost wires worker runtime requests into WanixSystem facade", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const worker = new FakeWorkerTarget();
    const requests = [];
    const systemElement = {
        ready: Promise.resolve(),
        system: {
            readText() {
                return "";
            },
            writeText() {},
            setupNamespace() {},
            handleRuntimeRequest(payload) {
                requests.push(payload);
                return new Uint8Array([payload[0] + 1]);
            },
        },
    };

    const host = await spawnBrowserWorkerHost(() => worker, { system: systemElement });
    t.after(() => host.close());

    const runtimePort = worker.posted[0].transfer[0];
    const responses = [];
    runtimePort.addEventListener("message", (event) => {
        responses.push(decodeWorkerRuntimeEnvelope(event.data));
    });
    runtimePort.start();

    runtimePort.postMessage(encodeWorkerRuntimeEnvelope("request", new Uint8Array([41])));

    assert.equal(requests.length, 1);
    assert.deepEqual(requests[0], new Uint8Array([41]));
    assert.equal(responses.length, 1);
    assert.equal(responses[0].kind, "response");
    assert.deepEqual(responses[0].payload, new Uint8Array([42]));
});

test("BrowserWorkerHost observes task messages and cleans up owned worker targets", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const worker = new FakeWorkerTarget();
    const host = await spawnBrowserWorkerHost(() => worker, {});

    const runtimePort = worker.posted[0].transfer[0];
    runtimePort.start();

    const taskMessages = [];
    host.onTaskMessage((message) => {
        taskMessages.push(message);
    });

    runtimePort.postMessage(encodeWorkerRuntimeEnvelope("task", new Uint8Array([4, 2, 4, 2])));
    assert.equal(taskMessages.length, 1);
    assert.equal(taskMessages[0].kind, "task");
    assert.deepEqual(taskMessages[0].payload, new Uint8Array([4, 2, 4, 2]));

    host.stop();
    assert.equal(host.started, false);
    runtimePort.postMessage(encodeWorkerRuntimeEnvelope("task", new Uint8Array([8, 8])));
    assert.equal(taskMessages.length, 1);

    await host.start();
    assert.equal(host.started, true);
    runtimePort.postMessage(encodeWorkerRuntimeEnvelope("task", new Uint8Array([6, 6])));
    assert.equal(taskMessages.length, 2);
    assert.deepEqual(taskMessages[1].payload, new Uint8Array([6, 6]));

    const localPort = host.port;
    host.close();

    assert.equal(host.closed, true);
    assert.equal(host.started, false);
    assert.equal(worker.terminateCalls, 1);
    assert.equal(localPort.closed, true);

    runtimePort.postMessage(encodeWorkerRuntimeEnvelope("task", new Uint8Array([0])));
    assert.equal(taskMessages.length, 2);

    host.close();
    assert.equal(worker.terminateCalls, 1);
});

test("BrowserWorkerHost exposes ordinary worker messages for export handoff", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const worker = new FakeWorkerTarget();
    const host = await spawnBrowserWorkerHost(() => worker, {});
    t.after(() => host.close());

    const targetMessages = [];
    const cleanup = host.onTargetMessage((event) => targetMessages.push(event.data));
    worker.dispatchMessage({ export: "port-placeholder", vm: "7" });

    assert.deepEqual(targetMessages, [{ export: "port-placeholder", vm: "7" }]);
    cleanup();
    worker.dispatchMessage({ export: "ignored" });
    assert.equal(targetMessages.length, 1);
});

test("callWanixLogger is a no-op by default and contains logger failures", async (t) => {
    const originalHTMLElement = globalThis.HTMLElement;
    globalThis.HTMLElement = class HTMLElement {};
    t.after(() => {
        if (originalHTMLElement === undefined) {
            delete globalThis.HTMLElement;
        } else {
            globalThis.HTMLElement = originalHTMLElement;
        }
    });
    const { callWanixLogger } = await import("../crates/wanix-web/js/system.js");
    const calls = [];
    assert.equal(callWanixLogger(null, "readText", ["file"]), false);
    assert.equal(
        callWanixLogger((operation, path) => calls.push([operation, path]), "readText", ["file"]),
        true,
    );
    assert.deepEqual(calls, [["readText", "file"]]);
    assert.equal(
        callWanixLogger(() => {
            throw new Error("logger failed");
        }, "writeText", ["file"]),
        false,
    );
});

test("createWanixRuntimeNamespaceClient sends typed namespace and fd requests over runtime endpoint", async () => {
    const endpoint = new FakeRuntimeEndpoint();
    const client = createWanixRuntimeNamespaceClient(endpoint, {
        taskId: "task-9",
        encodeRequest: encodeJsonBytes,
        decodeResponse: decodeJsonBytes,
        timeoutMs: 1000,
    });

    const pathRead = client.pathRead("work/input.txt");
    assert.equal(endpoint.requests.length, 1);
    assert.deepEqual(decodeJsonBytes(endpoint.requests[0]), {
        method: "PathRead",
        args: {
            task_id: "task-9",
            path: "work/input.txt",
        },
    });
    endpoint.respond({ type: "Bytes", value: [115, 101, 101, 100] });
    assert.deepEqual(await pathRead, { type: "Bytes", value: [115, 101, 101, 100] });

    const fdWrite = client.fdWrite(4, new Uint8Array([1, 2, 3]));
    assert.deepEqual(decodeJsonBytes(endpoint.requests[1]), {
        method: "FdWrite",
        args: {
            task_id: "task-9",
            fd: 4,
            data: [1, 2, 3],
        },
    });
    endpoint.respond({ type: "Count", value: 3 });
    assert.deepEqual(await fdWrite, { type: "Count", value: 3 });

    client.close();
});

function createSystemFacade() {
    return {
        readText() {
            return "";
        },
        writeText() {},
        setupNamespace() {},
    };
}

function encodeJsonBytes(value) {
    return new TextEncoder().encode(JSON.stringify(value, (_key, item) => {
        if (item instanceof Uint8Array) {
            return [...item];
        }
        return item;
    }));
}

function decodeJsonBytes(bytes) {
    return JSON.parse(new TextDecoder().decode(bytes));
}

class FakeRuntimeEndpoint {
    constructor() {
        this.requests = [];
        this._responseListeners = new Set();
        this._errorListeners = new Set();
    }

    sendRequest(payload) {
        this.requests.push(payload);
        return payload.byteLength;
    }

    onResponse(listener) {
        this._responseListeners.add(listener);
        return () => this._responseListeners.delete(listener);
    }

    onError(listener) {
        this._errorListeners.add(listener);
        return () => this._errorListeners.delete(listener);
    }

    respond(value) {
        const payload = encodeJsonBytes(value);
        for (const listener of this._responseListeners) {
            listener({ payload });
        }
    }
}

function installFakeMessageChannel() {
    const original = globalThis.MessageChannel;
    globalThis.MessageChannel = FakeMessageChannel;
    return () => {
        if (original === undefined) {
            delete globalThis.MessageChannel;
            return;
        }
        globalThis.MessageChannel = original;
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

class FakeWorkerTarget {
    constructor() {
        this.posted = [];
        this.terminateCalls = 0;
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

    postMessage(message, transfer = []) {
        this.posted.push({ message, transfer });
    }

    terminate() {
        this.terminateCalls += 1;
    }

    dispatchMessage(data) {
        for (const listener of this._listeners.get("message") ?? []) {
            listener({ data });
        }
    }
}
