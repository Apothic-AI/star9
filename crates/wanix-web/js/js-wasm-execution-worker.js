import {
    createJsWasmExecutionBootstrap,
    DEFAULT_JS_WASM_BOOTSTRAP_MESSAGE_TYPE,
} from "./js-wasm-worker-host.js";
import {
    acceptRuntimePort,
    DEFAULT_RUNTIME_PORT_MESSAGE_TYPE,
} from "./worker-runtime.js";

export const DEFAULT_JS_WASM_EXIT_TASK_MESSAGE_TYPE = "wanix-js-wasm-execution-exit";
export const DEFAULT_JS_WASM_ERROR_TASK_MESSAGE_TYPE = "wanix-js-wasm-execution-error";

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
        throw new Error(
            `direct WASM execution is not supported for ${JSON.stringify(String(specifier))}`,
        );
    }
    return import(String(specifier));
}

function defaultResolveModuleRunner(moduleRecord) {
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

function createDeferred() {
    let resolve;
    let reject;
    const promise = new Promise((promiseResolve, promiseReject) => {
        resolve = promiseResolve;
        reject = promiseReject;
    });
    return { promise, resolve, reject };
}
