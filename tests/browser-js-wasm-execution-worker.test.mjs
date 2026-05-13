import test from "node:test";
import assert from "node:assert/strict";

import {
    acceptJsWasmExecutionWorker,
    DEFAULT_JS_WASM_ERROR_TASK_MESSAGE_TYPE,
    DEFAULT_JS_WASM_EXIT_TASK_MESSAGE_TYPE,
    runJsWasmExecutionBootstrap,
} from "../crates/wanix-web/js/js-wasm-execution-worker.js";
import { DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE } from "../crates/wanix-web/js/js-wasm-worker-host.js";
import {
    DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
    decodeWorkerRuntimeEnvelope,
    WorkerRuntimeEndpoint,
} from "../crates/wanix-web/js/worker-runtime.js";

for (const order of ["runtime-first", "bootstrap-first"]) {
    test(`acceptJsWasmExecutionWorker waits for runtime and bootstrap (${order})`, async () => {
        const scope = new FakeWorkerScope();
        const channel = new FakeMessageChannel();
        const workerPort = channel.port1;
        const remotePort = channel.port2;
        const taskMessages = [];
        const runnerContexts = [];

        remotePort.start();
        remotePort.addEventListener("message", (event) => {
            taskMessages.push(decodeWorkerRuntimeEnvelope(event.data));
        });

        const handle = acceptJsWasmExecutionWorker(scope, {
            runner: async (context) => {
                runnerContexts.push(snapshotContext(context));
                context.sendTaskText("stdout line");
                context.sendTaskBinary(new Uint8Array([1, 2, 3, 4]));
                return { exitCode: 7 };
            },
        });

        const runtimeMessage = {
            type: DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
            task_id: "task-17",
            worker_id: "worker-17",
            descriptor: { port_id: "runtime", name: "runtime" },
            port: workerPort,
        };
        const bootstrapMessage = {
            type: DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE,
            task_id: "task-17",
            worker_id: "worker-17",
            kind: "js_wasm",
            module: "/bin/demo.js",
            args: ["--flag", 9],
            env: { TERM: "xterm-256color", DEBUG: 1 },
            cwd: "/workspace",
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
        };

        if (order === "runtime-first") {
            scope.dispatchMessage(runtimeMessage);
            await flushTasks();
            assert.equal(runnerContexts.length, 0);
            scope.dispatchMessage(bootstrapMessage);
        } else {
            scope.dispatchMessage(bootstrapMessage);
            await flushTasks();
            assert.equal(runnerContexts.length, 0);
            scope.dispatchMessage(runtimeMessage);
        }

        const result = await handle.promise;

        assert.equal(result.exitCode, 7);
        assert.equal(result.bootstrap.module, "/bin/demo.js");
        assert.equal(runnerContexts.length, 1);
        assert.deepEqual(runnerContexts[0], {
            taskId: "task-17",
            workerId: "worker-17",
            kind: "js_wasm",
            module: "/bin/demo.js",
            args: ["--flag", "9"],
            env: [
                { name: "TERM", value: "xterm-256color" },
                { name: "DEBUG", value: "1" },
            ],
            envMap: {
                TERM: "xterm-256color",
                DEBUG: "1",
            },
            cwd: "/workspace",
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
            runtimeDescriptor: { port_id: "runtime", name: "runtime" },
            runtimeMessage: {
                type: DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
                task_id: "task-17",
                worker_id: "worker-17",
                descriptor: { port_id: "runtime", name: "runtime" },
                port: "[MessagePort]",
            },
        });
        assert.equal(scope.listenerCount("message"), 0);
        assert.equal(taskMessages.length, 3);
        assert.equal(taskMessages[0].kind, "task");
        assert.equal(new TextDecoder().decode(taskMessages[0].payload), "stdout line");
        assert.deepEqual(taskMessages[1].payload, new Uint8Array([1, 2, 3, 4]));
        assert.deepEqual(
            JSON.parse(new TextDecoder().decode(taskMessages[2].payload)),
            {
                type: DEFAULT_JS_WASM_EXIT_TASK_MESSAGE_TYPE,
                task_id: "task-17",
                worker_id: "worker-17",
                kind: "js_wasm",
                module: "/bin/demo.js",
                exit_code: 7,
            },
        );
    });
}

test("runJsWasmExecutionBootstrap posts an error task message and rejects", async () => {
    const channel = new FakeMessageChannel();
    const endpoint = new WorkerRuntimeEndpoint(channel.port1, { autoStart: true });
    const taskMessages = [];

    channel.port2.start();
    channel.port2.addEventListener("message", (event) => {
        taskMessages.push(decodeWorkerRuntimeEnvelope(event.data));
    });

    await assert.rejects(
        () =>
            runJsWasmExecutionBootstrap(
                {
                    type: DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE,
                    task_id: "task-29",
                    worker_id: "worker-29",
                    module: "/bin/fail.js",
                    env: {},
                },
                {
                    runtimeEndpoint: endpoint,
                    runner: async () => {
                        throw new TypeError("runner exploded");
                    },
                },
            ),
        /runner exploded/,
    );

    assert.equal(taskMessages.length, 1);
    const errorPayload = JSON.parse(new TextDecoder().decode(taskMessages[0].payload));
    assert.equal(errorPayload.type, DEFAULT_JS_WASM_ERROR_TASK_MESSAGE_TYPE);
    assert.equal(errorPayload.task_id, "task-29");
    assert.equal(errorPayload.worker_id, "worker-29");
    assert.equal(errorPayload.kind, "js_wasm");
    assert.equal(errorPayload.module, "/bin/fail.js");
    assert.equal(errorPayload.name, "TypeError");
    assert.equal(errorPayload.message, "runner exploded");
    assert.match(errorPayload.stack, /TypeError: runner exploded/);
});

test("acceptJsWasmExecutionWorker cleanup removes the listener and stops future handling", async () => {
    const scope = new FakeWorkerScope();
    const channel = new FakeMessageChannel();
    let runnerCalls = 0;

    const handle = acceptJsWasmExecutionWorker(scope, {
        runner: async () => {
            runnerCalls += 1;
            return 0;
        },
    });

    handle.cleanup();

    scope.dispatchMessage({
        type: DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
        descriptor: { port_id: "runtime", name: "runtime" },
        port: channel.port1,
    });
    scope.dispatchMessage({
        type: DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE,
        module: "/bin/ignored.js",
    });
    await flushTasks();

    assert.equal(scope.listenerCount("message"), 0);
    assert.equal(runnerCalls, 0);
    assert.equal(handle.running, false);
    assert.equal(handle.settled, false);
});

function snapshotContext(context) {
    return {
        taskId: context.taskId,
        workerId: context.workerId,
        kind: context.kind,
        module: context.module,
        args: context.args.slice(),
        env: context.env.map((entry) => ({ ...entry })),
        envMap: { ...context.envMap },
        cwd: context.cwd,
        stdio: structuredCloneCompat(context.stdio),
        fds: context.fds.map((entry) => ({ ...entry })),
        ports: context.ports.map((entry) => ({ ...entry })),
        runtime: structuredCloneCompat(context.runtime),
        runtimeDescriptor: structuredCloneCompat(context.runtimeDescriptor),
        runtimeMessage: snapshotRuntimeMessage(context.runtimeMessage),
    };
}

function snapshotRuntimeMessage(message) {
    if (!message || typeof message !== "object") {
        return message;
    }
    return {
        ...message,
        descriptor: structuredCloneCompat(message.descriptor),
        port: "port" in message ? "[MessagePort]" : undefined,
    };
}

function structuredCloneCompat(value) {
    if (typeof structuredClone === "function") {
        return structuredClone(value);
    }
    return JSON.parse(JSON.stringify(value));
}

async function flushTasks() {
    await Promise.resolve();
    await Promise.resolve();
}

class FakeWorkerScope {
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
        const event = { data };
        for (const listener of this._listeners.get("message") ?? []) {
            listener(event);
        }
    }

    listenerCount(type) {
        return this._listeners.get(type)?.size ?? 0;
    }
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
