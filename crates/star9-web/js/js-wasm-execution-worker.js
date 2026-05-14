import {
    createJsWasmExecutionBootstrap,
    DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE,
} from "./js-wasm-worker-host.js";
import {
    acceptRuntimePort,
    DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
} from "./worker-runtime.js";

export const DEFAULT_JS_WASM_EXIT_TASK_MESSAGE_TYPE = "star9-js-wasm-execution-exit";
export const DEFAULT_JS_WASM_ERROR_TASK_MESSAGE_TYPE = "star9-js-wasm-execution-error";
const DIRECT_WASM_MODULE_RECORD = Symbol("star9.directWasmModule");
const WASI_ERRNO_SUCCESS = 0;
const WASI_ERRNO_BADF = 8;
const WASI_ERRNO_INVAL = 28;
const WASI_ERRNO_NOTSUP = 58;
const WASI_CLOCK_REALTIME = 0;
const WASI_CLOCK_MONOTONIC = 1;

export function acceptJsWasmExecutionWorker(scope, options = {}) {
    const workerScope = requireWorkerScope(scope);
    const runtimeMessageType =
        normalizeOptionalString(options.runtimeMessageType ?? options.runtime_message_type) ||
        DEFAULT_RUNTIME_PORT_MESSAGE_TYPE;
    const bootstrapType =
        normalizeOptionalString(options.bootstrapType ?? options.bootstrap_message_type) ||
        DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE;
    const executionOptions = {
        ...options,
        runtimeMessageType,
        bootstrapType,
    };

    let active = true;
    let runtime = null;
    let bootstrap = null;
    let running = false;
    let settled = false;

    const deferred = createDeferred();
    const handle = {
        get runtime() {
            return runtime;
        },
        get bootstrap() {
            return bootstrap;
        },
        get running() {
            return running;
        },
        get settled() {
            return settled;
        },
        promise: deferred.promise,
        cleanup() {
            if (!active) {
                return handle;
            }
            active = false;
            workerScope.removeEventListener("message", onMessage);
            runtime?.endpoint?.stop();
            return handle;
        },
        stop() {
            return handle.cleanup();
        },
        dispose() {
            return handle.cleanup();
        },
    };

    workerScope.addEventListener("message", onMessage);

    return handle;

    function onMessage(event) {
        if (!active || settled || running) {
            return;
        }

        const payload = unwrapMessageEvent(event);
        if (!payload || typeof payload !== "object") {
            return;
        }

        if (payload.type === runtimeMessageType && runtime == null) {
            runtime = acceptRuntimePort(event, {
                endpoint: {
                    autoStart: false,
                    ...(options.endpoint || {}),
                },
            });
        } else if (payload.type === bootstrapType && bootstrap == null) {
            bootstrap = createJsWasmExecutionBootstrap(payload);
        } else {
            return;
        }

        if (!runtime || !bootstrap || running) {
            return;
        }

        running = true;
        workerScope.removeEventListener("message", onMessage);
        runtime.endpoint.start();

        Promise.resolve(
            runJsWasmExecutionBootstrap(bootstrap, {
                ...executionOptions,
                runtimeEndpoint: runtime.endpoint,
                runtimePort: runtime.port,
                runtimeDescriptor: runtime.descriptor,
                runtimeMessage: runtime.message,
                scope: workerScope,
            }),
        ).then(
            (result) => {
                settled = true;
                active = false;
                runtime.endpoint.stop();
                deferred.resolve(result);
            },
            (error) => {
                settled = true;
                active = false;
                runtime.endpoint.stop();
                deferred.reject(error);
            },
        );
    }
}

export async function runJsWasmExecutionBootstrap(bootstrap, options = {}) {
    const normalizedBootstrap = createJsWasmExecutionBootstrap(bootstrap);
    const runtimeEndpoint = requireRuntimeEndpoint(
        options.runtimeEndpoint ?? options.runtime ?? options.endpoint,
    );
    const executionContext = createExecutionContext(normalizedBootstrap, runtimeEndpoint, options);

    try {
        const runner = await resolveExecutionRunner(executionContext, options);
        const result = await runner(executionContext);
        const exitCode = normalizeExitCode(extractExitCode(result));
        if (!executionContext.state.exitSent) {
            executionContext.sendExitTaskMessage(exitCode);
        }
        return {
            bootstrap: normalizedBootstrap,
            context: executionContext,
            result,
            exitCode,
        };
    } catch (error) {
        const executionError = normalizeExecutionError(error);
        if (!executionContext.state.errorSent) {
            executionContext.sendErrorTaskMessage(executionError);
        }
        throw executionError;
    }
}

async function resolveExecutionRunner(context, options) {
    if (typeof options.runner === "function") {
        return options.runner;
    }

    const loadModule =
        typeof options.loadModule === "function" ? options.loadModule : defaultLoadExecutionModule;
    const moduleRecord = await loadModule(context.module, context);
    const resolveRunner =
        typeof options.resolveRunner === "function"
            ? options.resolveRunner
            : defaultResolveModuleRunner;
    const runner = resolveRunner(moduleRecord, context);
    if (typeof runner !== "function") {
        throw new TypeError("expected JS/WASM execution runner to be a function");
    }
    return runner;
}

async function defaultLoadExecutionModule(specifier) {
    if (looksLikeWasmModule(specifier)) {
        return loadDirectWasmModule(specifier);
    }
    return import(String(specifier));
}

function defaultResolveModuleRunner(moduleRecord) {
    if (moduleRecord?.[DIRECT_WASM_MODULE_RECORD]) {
        return (context) => runDirectWasmModule(moduleRecord, context);
    }
    if (typeof moduleRecord === "function") {
        return moduleRecord;
    }
    if (typeof moduleRecord?.default === "function") {
        return moduleRecord.default;
    }
    if (typeof moduleRecord?.run === "function") {
        return moduleRecord.run;
    }
    throw new TypeError("expected module to export a default or named run function");
}

async function loadDirectWasmModule(specifier) {
    if (typeof WebAssembly === "undefined" || typeof WebAssembly.compile !== "function") {
        throw new Error("direct WASM execution requires WebAssembly.compile");
    }
    const source = String(specifier);
    const bytes = await loadWasmBytes(source);
    return {
        [DIRECT_WASM_MODULE_RECORD]: true,
        source,
        module: await WebAssembly.compile(bytes),
    };
}

async function loadWasmBytes(source) {
    if (source.startsWith("file:")) {
        const { readFile } = await import("node:fs/promises");
        return new Uint8Array(await readFile(new URL(source)));
    }
    const response = await fetch(source);
    if (!response.ok) {
        throw new Error(`failed to fetch WASM module ${JSON.stringify(source)}: ${response.status}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

async function runDirectWasmModule(moduleRecord, context) {
    const state = {
        context,
        exitCode: null,
        memory: null,
        textEncoder: getTextEncoder(),
        textDecoder: getTextDecoder(),
    };
    const imports = createBrowserWasiImports(state);
    const instance = await WebAssembly.instantiate(moduleRecord.module, imports);
    state.memory = instance.exports.memory;
    if (!state.memory || !(state.memory.buffer instanceof ArrayBuffer)) {
        throw new Error("direct WASM module must export linear memory");
    }

    try {
        if (typeof instance.exports._start === "function") {
            instance.exports._start();
        } else if (typeof instance.exports.run === "function") {
            state.exitCode = extractExitCode(instance.exports.run());
        } else if (typeof instance.exports.main === "function") {
            state.exitCode = extractExitCode(instance.exports.main());
        } else {
            throw new Error("direct WASM module must export _start, run, or main");
        }
    } catch (error) {
        if (error instanceof WasiProcExit) {
            state.exitCode = error.code;
        } else {
            throw error;
        }
    }

    return {
        exitCode: normalizeExitCode(state.exitCode ?? 0),
        wasm: {
            source: moduleRecord.source,
        },
    };
}

class WasiProcExit extends Error {
    constructor(code) {
        super(`WASI proc_exit(${normalizeExitCode(code)})`);
        this.name = "WasiProcExit";
        this.code = normalizeExitCode(code);
    }
}

function createBrowserWasiImports(state) {
    const args = [state.context.module, ...state.context.args];
    const env = state.context.env.map((entry) => `${entry.name}=${entry.value}`);

    return {
        wasi_snapshot_preview1: {
            args_sizes_get(countPtr, sizePtr) {
                return writeStringArraySizes(state, args, countPtr, sizePtr);
            },
            args_get(argvPtr, argvBufPtr) {
                return writeStringArray(state, args, argvPtr, argvBufPtr);
            },
            environ_sizes_get(countPtr, sizePtr) {
                return writeStringArraySizes(state, env, countPtr, sizePtr);
            },
            environ_get(envPtr, envBufPtr) {
                return writeStringArray(state, env, envPtr, envBufPtr);
            },
            clock_res_get(clockId, resolutionPtr) {
                if (!isSupportedClock(clockId)) {
                    return WASI_ERRNO_INVAL;
                }
                writeU64(state, resolutionPtr, 1_000_000n);
                return WASI_ERRNO_SUCCESS;
            },
            clock_time_get(clockId, _precision, timePtr) {
                if (!isSupportedClock(clockId)) {
                    return WASI_ERRNO_INVAL;
                }
                writeU64(state, timePtr, currentNanos(clockId));
                return WASI_ERRNO_SUCCESS;
            },
            random_get(bufPtr, bufLen) {
                if (bufLen < 0) {
                    return WASI_ERRNO_INVAL;
                }
                const bytes = new Uint8Array(bufLen);
                if (globalThis.crypto?.getRandomValues) {
                    globalThis.crypto.getRandomValues(bytes);
                } else {
                    for (let index = 0; index < bytes.length; index += 1) {
                        bytes[index] = (index * 31 + 17) & 0xff;
                    }
                }
                writeBytes(state, bufPtr, bytes);
                return WASI_ERRNO_SUCCESS;
            },
            fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {
                if (fd < 0 || iovsLen < 0) {
                    return WASI_ERRNO_BADF;
                }
                const chunks = [];
                let written = 0;
                for (let index = 0; index < iovsLen; index += 1) {
                    const ptr = readU32(state, iovsPtr + index * 8);
                    const len = readU32(state, iovsPtr + index * 8 + 4);
                    const chunk = readBytes(state, ptr, len);
                    chunks.push(chunk);
                    written += chunk.byteLength;
                }
                const data = concatBytes(chunks);
                if (fd === 1 || fd === 2) {
                    state.context.sendTaskText(state.textDecoder.decode(data));
                } else {
                    state.context.sendTaskBinary(data);
                }
                writeU32(state, nwrittenPtr, written);
                return WASI_ERRNO_SUCCESS;
            },
            fd_read(_fd, _iovsPtr, _iovsLen, nreadPtr) {
                writeU32(state, nreadPtr, 0);
                return WASI_ERRNO_SUCCESS;
            },
            fd_close() {
                return WASI_ERRNO_SUCCESS;
            },
            fd_seek(_fd, _offset, _whence, newOffsetPtr) {
                writeU64(state, newOffsetPtr, 0n);
                return WASI_ERRNO_SUCCESS;
            },
            fd_tell(_fd, offsetPtr) {
                writeU64(state, offsetPtr, 0n);
                return WASI_ERRNO_SUCCESS;
            },
            fd_fdstat_get(_fd, statPtr) {
                writeBytes(state, statPtr, new Uint8Array(24));
                return WASI_ERRNO_SUCCESS;
            },
            fd_prestat_get(_fd, prestatPtr) {
                writeU32(state, prestatPtr, 0);
                writeU32(state, prestatPtr + 4, 1);
                return WASI_ERRNO_SUCCESS;
            },
            fd_prestat_dir_name(_fd, pathPtr, pathLen) {
                if (pathLen < 1) {
                    return WASI_ERRNO_INVAL;
                }
                writeBytes(state, pathPtr, state.textEncoder.encode("/"));
                return WASI_ERRNO_SUCCESS;
            },
            poll_oneoff(_inPtr, _outPtr, nsubscriptions, neventsPtr) {
                if (nsubscriptions <= 0) {
                    return WASI_ERRNO_INVAL;
                }
                writeU32(state, neventsPtr, 0);
                return WASI_ERRNO_SUCCESS;
            },
            sched_yield() {
                return WASI_ERRNO_SUCCESS;
            },
            proc_raise() {
                return WASI_ERRNO_NOTSUP;
            },
            proc_exit(code) {
                throw new WasiProcExit(code);
            },
        },
    };
}

function writeStringArraySizes(state, values, countPtr, sizePtr) {
    writeU32(state, countPtr, values.length);
    writeU32(
        state,
        sizePtr,
        values.reduce((sum, value) => sum + state.textEncoder.encode(String(value)).byteLength + 1, 0),
    );
    return WASI_ERRNO_SUCCESS;
}

function writeStringArray(state, values, ptrsPtr, bufPtr) {
    let cursor = bufPtr;
    for (let index = 0; index < values.length; index += 1) {
        const bytes = state.textEncoder.encode(String(values[index]));
        writeU32(state, ptrsPtr + index * 4, cursor);
        writeBytes(state, cursor, bytes);
        cursor += bytes.byteLength;
        writeBytes(state, cursor, new Uint8Array([0]));
        cursor += 1;
    }
    return WASI_ERRNO_SUCCESS;
}

function isSupportedClock(clockId) {
    return clockId === WASI_CLOCK_REALTIME || clockId === WASI_CLOCK_MONOTONIC;
}

function currentNanos(clockId) {
    if (clockId === WASI_CLOCK_MONOTONIC && globalThis.performance?.now) {
        return BigInt(Math.floor(globalThis.performance.now() * 1_000_000));
    }
    return BigInt(Date.now()) * 1_000_000n;
}

function readU32(state, ptr) {
    return memoryView(state).getUint32(ptr, true);
}

function writeU32(state, ptr, value) {
    memoryView(state).setUint32(ptr, Number(value) >>> 0, true);
}

function writeU64(state, ptr, value) {
    memoryView(state).setBigUint64(ptr, BigInt(value), true);
}

function readBytes(state, ptr, len) {
    return new Uint8Array(state.memory.buffer, ptr, len).slice();
}

function writeBytes(state, ptr, bytes) {
    new Uint8Array(state.memory.buffer, ptr, bytes.byteLength).set(bytes);
}

function memoryView(state) {
    return new DataView(state.memory.buffer);
}

function concatBytes(chunks) {
    const out = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.byteLength, 0));
    let offset = 0;
    for (const chunk of chunks) {
        out.set(chunk, offset);
        offset += chunk.byteLength;
    }
    return out;
}

function createExecutionContext(bootstrap, runtimeEndpoint, options) {
    const textEncoder = getTextEncoder();
    const taskId = normalizeOptionalString(bootstrap.task_id);
    const workerId = normalizeOptionalString(bootstrap.worker_id);
    const state = {
        exitSent: false,
        errorSent: false,
    };

    return {
        bootstrap,
        state,
        scope: options.scope ?? null,
        runtimeEndpoint,
        runtimePort: options.runtimePort ?? null,
        runtimeDescriptor: cloneRecord(options.runtimeDescriptor),
        runtimeMessage: cloneRuntimeMessage(options.runtimeMessage),
        runtime: cloneRecord(bootstrap.runtime),
        kind: bootstrap.kind,
        module: bootstrap.module,
        taskId,
        workerId,
        args: bootstrap.args.slice(),
        env: bootstrap.env.map((entry) => ({ ...entry })),
        envMap: Object.fromEntries(bootstrap.env.map((entry) => [entry.name, entry.value])),
        cwd: normalizeOptionalString(bootstrap.cwd),
        stdio: cloneRecord(bootstrap.stdio) || {},
        fds: bootstrap.fds.map((entry) => ({ ...entry })),
        ports: bootstrap.ports.map((entry) => ({ ...entry })),
        sendTaskBinary(data) {
            return runtimeEndpoint.sendTaskMessage(toUint8Array(data, "task message payload"));
        },
        sendTaskText(text) {
            return runtimeEndpoint.sendTaskMessage(textEncoder.encode(String(text)));
        },
        sendExitTaskMessage(exitCode = 0) {
            state.exitSent = true;
            return runtimeEndpoint.sendTaskMessage(
                textEncoder.encode(
                    JSON.stringify({
                        type: DEFAULT_JS_WASM_EXIT_TASK_MESSAGE_TYPE,
                        task_id: taskId,
                        worker_id: workerId,
                        kind: bootstrap.kind,
                        module: bootstrap.module,
                        exit_code: normalizeExitCode(exitCode),
                    }),
                ),
            );
        },
        sendErrorTaskMessage(error) {
            state.errorSent = true;
            const details = serializeExecutionError(error);
            return runtimeEndpoint.sendTaskMessage(
                textEncoder.encode(
                    JSON.stringify({
                        type: DEFAULT_JS_WASM_ERROR_TASK_MESSAGE_TYPE,
                        task_id: taskId,
                        worker_id: workerId,
                        kind: bootstrap.kind,
                        module: bootstrap.module,
                        ...details,
                    }),
                ),
            );
        },
    };
}

function extractExitCode(result) {
    if (result == null) {
        return 0;
    }
    if (typeof result === "number" || typeof result === "bigint") {
        return result;
    }
    if (typeof result !== "object") {
        return 0;
    }
    if ("exitCode" in result) {
        return result.exitCode;
    }
    if ("code" in result) {
        return result.code;
    }
    return 0;
}

function normalizeExitCode(value) {
    if (value == null) {
        return 0;
    }
    const code = Number(value);
    if (!Number.isFinite(code) || !Number.isInteger(code)) {
        throw new TypeError("expected exit code to be a finite integer");
    }
    return code;
}

function normalizeExecutionError(error) {
    if (error instanceof Error) {
        return error;
    }
    return new Error(typeof error === "string" ? error : JSON.stringify(error));
}

function serializeExecutionError(error) {
    const normalized = normalizeExecutionError(error);
    const details = {
        name: normalized.name || "Error",
        message: normalized.message || "Unknown execution error",
    };
    if (typeof normalized.stack === "string" && normalized.stack.length > 0) {
        details.stack = normalized.stack;
    }
    if ("code" in normalized && normalized.code != null) {
        details.code = normalized.code;
    }
    return details;
}

function requireWorkerScope(scope) {
    if (
        !scope ||
        typeof scope.addEventListener !== "function" ||
        typeof scope.removeEventListener !== "function"
    ) {
        throw new TypeError(
            "expected a WorkerGlobalScope-like object with addEventListener/removeEventListener",
        );
    }
    return scope;
}

function requireRuntimeEndpoint(endpoint) {
    if (
        !endpoint ||
        typeof endpoint.start !== "function" ||
        typeof endpoint.stop !== "function" ||
        typeof endpoint.sendTaskMessage !== "function"
    ) {
        throw new TypeError("expected a WorkerRuntimeEndpoint-like object");
    }
    return endpoint;
}

function unwrapMessageEvent(event) {
    if (event && typeof event === "object" && "data" in event) {
        return event.data;
    }
    return event;
}

function normalizeOptionalString(value) {
    if (value == null) {
        return null;
    }
    const normalized = String(value);
    return normalized.length > 0 ? normalized : null;
}

function cloneRecord(value) {
    if (value == null) {
        return null;
    }
    if (Array.isArray(value) || typeof value !== "object") {
        throw new TypeError("expected record values to be objects");
    }
    return structuredCloneCompat(value);
}

function cloneRuntimeMessage(message) {
    if (message == null) {
        return null;
    }
    if (Array.isArray(message) || typeof message !== "object") {
        throw new TypeError("expected runtime message values to be objects");
    }

    const clone = { ...message };
    if ("descriptor" in clone) {
        clone.descriptor = cloneRecord(clone.descriptor);
    }
    return clone;
}

function structuredCloneCompat(value) {
    if (typeof structuredClone === "function") {
        return structuredClone(value);
    }
    return JSON.parse(JSON.stringify(value));
}

function toUint8Array(value, label) {
    if (value instanceof Uint8Array) {
        return value.slice();
    }
    if (typeof ArrayBuffer !== "undefined" && value instanceof ArrayBuffer) {
        return new Uint8Array(value.slice(0));
    }
    if (typeof SharedArrayBuffer !== "undefined" && value instanceof SharedArrayBuffer) {
        return new Uint8Array(value).slice();
    }
    if (ArrayBuffer.isView(value)) {
        return new Uint8Array(value.buffer, value.byteOffset, value.byteLength).slice();
    }
    throw new TypeError(`expected ${label} to be binary data`);
}

function looksLikeWasmModule(specifier) {
    return /\.wasm(?:$|[?#])/.test(String(specifier));
}

function getTextEncoder() {
    if (typeof TextEncoder !== "function") {
        throw new Error("TextEncoder is required for JS/WASM execution worker messages");
    }
    return new TextEncoder();
}

function getTextDecoder() {
    if (typeof TextDecoder !== "function") {
        throw new Error("TextDecoder is required for JS/WASM execution worker messages");
    }
    return new TextDecoder();
}

function createDeferred() {
    let resolve;
    let reject;
    const promise = new Promise((promiseResolve, promiseReject) => {
        resolve = promiseResolve;
        reject = promiseReject;
    });
    return { promise, resolve, reject };
}
