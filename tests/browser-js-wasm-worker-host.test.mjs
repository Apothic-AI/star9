import test from "node:test";
import assert from "node:assert/strict";

import {
    BrowserJsWasmWorkerHost,
    DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE,
    DEFAULT_JS_WASM_EXECUTION_KIND,
    attachBrowserJsWasmWorkerHost,
    createJsWasmExecutionBootstrap,
    spawnBrowserJsWasmWorkerHost,
} from "../crates/star9-web/js/js-wasm-worker-host.js";
import {
    DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
    encodeWorkerRuntimeEnvelope,
} from "../crates/star9-web/js/worker-runtime.js";
import { BrowserWorkerHost } from "../crates/star9-web/js/worker-host.js";

test("createJsWasmExecutionBootstrap normalizes execution fields", () => {
    const bootstrap = createJsWasmExecutionBootstrap({
        task_id: "task-1",
        worker_id: "worker-1",
        module: "/sys/main.wasm",
        args: ["--flag", 7],
        env: { TERM: "xterm-256color", DEBUG: 1 },
        cwd: "/work",
        stdio: {
            stdout: {
                kind: "port",
                value: { port_id: "stdout", name: "stdout" },
            },
        },
        fds: [{ fd: 3, kind: "pipe", read: true, write: false }],
        ports: [{ port_id: "control", name: "control" }],
        runtime: {
            type: DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
            descriptor: { port_id: "runtime", name: "runtime" },
        },
    });

    assert.equal(bootstrap.type, DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE);
    assert.equal(bootstrap.kind, DEFAULT_JS_WASM_EXECUTION_KIND);
    assert.equal(bootstrap.module, "/sys/main.wasm");
    assert.deepEqual(bootstrap.args, ["--flag", "7"]);
    assert.deepEqual(bootstrap.env, [
        { name: "TERM", value: "xterm-256color" },
        { name: "DEBUG", value: "1" },
    ]);
    assert.equal(bootstrap.cwd, "/work");
    assert.deepEqual(bootstrap.stdio, {
        stdout: {
            kind: "port",
            value: { port_id: "stdout", name: "stdout" },
        },
    });
    assert.deepEqual(bootstrap.fds, [{ fd: 3, kind: "pipe", read: true, write: false }]);
    assert.deepEqual(bootstrap.ports, [{ port_id: "control", name: "control" }]);
    assert.equal(bootstrap.task_id, "task-1");
    assert.equal(bootstrap.worker_id, "worker-1");
    assert.deepEqual(bootstrap.runtime, {
        type: DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
        descriptor: { port_id: "runtime", name: "runtime" },
    });
});

test("spawnBrowserJsWasmWorkerHost transfers the runtime port and posts a stable execution bootstrap", {
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

    const host = await spawnBrowserJsWasmWorkerHost(() => worker, {
        system: systemElement,
        taskId: "task-17",
        workerId: "worker-17",
        descriptor: "runtime",
        module: "/bin/demo.wasm",
        args: ["--color", "always"],
        env: [
            { name: "TERM", value: "xterm-256color" },
            { name: "DEBUG", value: "1" },
        ],
        cwd: "/workspace",
        stdio: {
            stdout: {
                kind: "port",
                value: { port_id: "stdout", name: "stdout" },
            },
        },
        fds: [{ fd: 3, kind: "file", path: "/tmp/log", read: true, write: true }],
        ports: [{ port_id: "control", name: "control" }],
        bootstrapMessage: { trace_id: "trace-17" },
    });
    t.after(() => host.close());

    assert.equal(host.started, true);
    assert.equal(host.target, worker);
    assert.equal(worker.posted.length, 2);

    const [runtimePost, bootstrapPost] = worker.posted;
    assert.equal(runtimePost.message.type, DEFAULT_RUNTIME_PORT_MESSAGE_TYPE);
    assert.equal(runtimePost.message.task_id, "task-17");
    assert.equal(runtimePost.message.worker_id, "worker-17");
    assert.equal(runtimePost.message.system_id, "system-under-test");
    assert.deepEqual(runtimePost.message.descriptor, { port_id: "runtime", name: "runtime" });
    assert.equal(runtimePost.transfer.length, 1);
    assert.equal(typeof runtimePost.transfer[0].postMessage, "function");

    assert.equal(bootstrapPost.transfer.length, 0);
    assert.equal(bootstrapPost.message.type, DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE);
    assert.equal(bootstrapPost.message.trace_id, "trace-17");
    assert.equal(bootstrapPost.message.task_id, "task-17");
    assert.equal(bootstrapPost.message.worker_id, "worker-17");
    assert.equal(bootstrapPost.message.kind, "js_wasm");
    assert.equal(bootstrapPost.message.module, "/bin/demo.wasm");
    assert.deepEqual(bootstrapPost.message.args, ["--color", "always"]);
    assert.deepEqual(bootstrapPost.message.env, [
        { name: "TERM", value: "xterm-256color" },
        { name: "DEBUG", value: "1" },
    ]);
    assert.equal(bootstrapPost.message.cwd, "/workspace");
    assert.deepEqual(bootstrapPost.message.stdio, {
        stdout: {
            kind: "port",
            value: { port_id: "stdout", name: "stdout" },
        },
    });
    assert.deepEqual(bootstrapPost.message.fds, [
        { fd: 3, kind: "file", path: "/tmp/log", read: true, write: true },
    ]);
    assert.deepEqual(bootstrapPost.message.ports, [
        { port_id: "control", name: "control" },
    ]);
    assert.deepEqual(bootstrapPost.message.runtime, {
        type: DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
        descriptor: { port_id: "runtime", name: "runtime" },
        system_id: "system-under-test",
    });
    assert.equal(host.facade, systemFacade);
    assert.equal(host.element, systemElement);
});

test("BrowserJsWasmWorkerHost delegates runtime task messages and cleanup through BrowserWorkerHost", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const worker = new FakeWorkerTarget();
    const host = await attachBrowserJsWasmWorkerHost(worker, {
        taskId: "task-23",
        workerId: "worker-23",
        module: "/bin/runtime.wasm",
    });
    t.after(() => host.close());

    const runtimePort = worker.posted[0].transfer[0];
    runtimePort.start();

    const taskMessages = [];
    host.onTaskMessage((message) => {
        taskMessages.push(message);
    });

    runtimePort.postMessage(encodeWorkerRuntimeEnvelope("task", new Uint8Array([69, 88, 73, 84])));

    assert.equal(taskMessages.length, 1);
    assert.equal(taskMessages[0].kind, "task");
    assert.deepEqual(taskMessages[0].payload, new Uint8Array([69, 88, 73, 84]));

    const localPort = host.port;
    host.stop();
    assert.equal(host.started, false);

    runtimePort.postMessage(encodeWorkerRuntimeEnvelope("task", new Uint8Array([0])));
    assert.equal(taskMessages.length, 1);

    await host.start();
    assert.equal(worker.posted.length, 2);
    runtimePort.postMessage(encodeWorkerRuntimeEnvelope("task", new Uint8Array([1, 2, 3])));
    assert.equal(taskMessages.length, 2);
    assert.deepEqual(taskMessages[1].payload, new Uint8Array([1, 2, 3]));

    host.close();
    assert.equal(host.closed, true);
    assert.equal(localPort.closed, true);
    assert.equal(worker.terminateCalls, 0);
});

test("BrowserJsWasmWorkerHost can wrap an existing BrowserWorkerHost instance", {
    concurrency: false,
}, async (t) => {
    const restore = installFakeMessageChannel();
    t.after(restore);

    const worker = new FakeWorkerTarget();
    const workerHost = BrowserWorkerHost.spawn(() => worker, {
        taskId: "task-29",
        workerId: "worker-29",
    });
    const host = new BrowserJsWasmWorkerHost({
        workerHost,
        module: "/bin/wrapped.wasm",
    });
    t.after(() => host.close());

    await host.start();

    assert.equal(worker.posted.length, 2);
    assert.equal(worker.posted[1].message.module, "/bin/wrapped.wasm");

    const localPort = host.port;
    host.close();
    assert.equal(worker.terminateCalls, 1);
    assert.equal(localPort.closed, true);
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
}
