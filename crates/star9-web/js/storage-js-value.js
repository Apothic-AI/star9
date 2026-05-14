const DEFAULT_STORAGE_MESSAGE_TYPE = "star9-storage";
const DEFAULT_TEXT_ENCODER = new TextEncoder();
const DEFAULT_TEXT_DECODER = new TextDecoder();

export async function createJsValueStorageAdapter(descriptor, options = {}) {
    const normalizedDescriptor = normalizeJsValueDescriptor(descriptor);
    const handle = resolveNamedHandle(normalizedDescriptor.value, {
        globals: options.globals,
        registry: options.registry,
        label: "js-value handle",
    });
    const root = resolveJsRoot(handle, normalizedDescriptor.path);
    const codec = createTextCodec(options);

    return {
        descriptor: normalizedDescriptor,
        async stat(path = ".") {
            const node = resolveNode(root, path, "stat");
            return describeNode(node, joinNormalizedPath(root.path, normalizeRelativePath(path)));
        },
        async readFile(path = ".") {
            const node = resolveNode(root, path, "readFile");
            return readNodeBytes(node, codec);
        },
        async writeFile(path, bytes) {
            const normalizedPath = normalizeRelativePath(path);
            const text = codec.decode(toUint8Array(bytes, "writeFile bytes"), true);
            if (normalizedPath === ".") {
                const node = resolveNode(root, ".", "writeFile");
                if (typeof node.assign !== "function") {
                    throw createNodeError("writeFile", node.displayPath, "Path is not writable");
                }
                node.assign(coerceAssignedValue(node.value, text));
                return;
            }
            const parentRef = resolveParent(root, normalizedPath, "writeFile");
            const current = lookupChild(parentRef.parentNode.value, parentRef.key);
            const nextValue = coerceAssignedValue(current.exists ? current.value : "", text);
            assignChild(parentRef, nextValue, "writeFile");
        },
        async readText(path = ".") {
            const bytes = await this.readFile(path);
            return codec.decode(bytes, false);
        },
        async writeText(path, text) {
            await this.writeFile(path, codec.encode(requireString(text, "writeText text")));
        },
        async readDir(path = ".") {
            const node = resolveNode(root, path, "readDir");
            if (!isDirectoryValue(node.value)) {
                throw createNodeError("readDir", node.displayPath, "Not a directory");
            }
            return listDirectoryEntries(node.value);
        },
        async mkdir(path) {
            const normalizedPath = normalizeRelativePath(path);
            if (normalizedPath === ".") {
                throw createNodeError("mkdir", root.path, "Path already exists");
            }
            const parentRef = resolveParent(root, normalizedPath, "mkdir");
            if (lookupChild(parentRef.parentNode.value, parentRef.key).exists) {
                throw createNodeError("mkdir", joinNormalizedPath(root.path, normalizedPath), "Path already exists");
            }
            assignChild(parentRef, {}, "mkdir");
        },
        async remove(path) {
            const normalizedPath = normalizeRelativePath(path);
            if (normalizedPath === ".") {
                removeSelf(root, "remove");
                return;
            }
            const parentRef = resolveParent(root, normalizedPath, "remove");
            removeChild(parentRef, "remove");
        },
    };
}

export async function createWorkerStorageAdapter(descriptor, options = {}) {
    const normalizedDescriptor = normalizeWorkerDescriptor(descriptor);
    const handle = resolveNamedHandle(normalizedDescriptor.worker, {
        globals: options.globals,
        registry: options.registry,
        label: "worker handle",
    });
    const target = requireMessageTarget(resolvePortLikeTarget(handle));
    const codec = createTextCodec(options);
    const pending = new Map();
    const messageType = String(options.messageType || DEFAULT_STORAGE_MESSAGE_TYPE);
    const requestIdPrefix = String(options.requestIdPrefix || `${normalizedDescriptor.worker}:`);
    const transferBinary = options.transferBinary !== false;
    const timeoutMs = options.timeoutMs ?? 0;
    let nextRequestId = 0;

    const onMessage = (event) => {
        const payload = unwrapMessageEvent(event);
        if (!isStorageResponseMessage(payload, messageType)) {
            return;
        }
        const pendingRequest = pending.get(String(payload.id ?? ""));
        if (!pendingRequest) {
            return;
        }
        pending.delete(pendingRequest.id);
        if (pendingRequest.timeout != null) {
            clearTimeout(pendingRequest.timeout);
        }
        if (payload.ok === false) {
            pendingRequest.reject(protocolError(payload.error, pendingRequest.op));
            return;
        }
        try {
            pendingRequest.resolve(normalizeWorkerResult(payload, pendingRequest.op));
        } catch (error) {
            pendingRequest.reject(error);
        }
    };

    target.addEventListener("message", onMessage);
    startMessageTarget(target);

    const request = async (op, path = ".", extra = {}) => {
        const normalizedPath = joinNormalizedPath(
            normalizedDescriptor.path,
            normalizeRelativePath(path),
        );
        const id = `${requestIdPrefix}${++nextRequestId}`;
        const message = {
            type: messageType,
            kind: "request",
            id,
            op,
            path: normalizedPath,
            ...extra,
        };
        const transfer = [];
        if (extra.bytes instanceof Uint8Array && transferBinary) {
            transfer.push(extra.bytes.buffer);
        }

        return new Promise((resolve, reject) => {
            const timeout =
                timeoutMs > 0
                    ? setTimeout(() => {
                        pending.delete(id);
                        reject(new Error(`${op} request ${id} timed out after ${timeoutMs}ms`));
                    }, timeoutMs)
                    : null;
            pending.set(id, {
                id,
                op,
                resolve,
                reject,
                timeout,
            });
            try {
                target.postMessage(message, transfer);
            } catch (error) {
                if (timeout != null) {
                    clearTimeout(timeout);
                }
                pending.delete(id);
                reject(new Error(`${op} request ${id} failed: ${errorMessage(error)}`));
            }
        });
    };

    return {
        descriptor: normalizedDescriptor,
        async stat(path = ".") {
            const result = await request("stat", path);
            return normalizeStatResult(result, joinNormalizedPath(normalizedDescriptor.path, normalizeRelativePath(path)));
        },
        async readFile(path = ".") {
            const result = await request("readFile", path);
            return extractBinaryResult(result, "readFile response bytes");
        },
        async writeFile(path, bytes) {
            await request("writeFile", path, {
                bytes: copyTransferableBytes(bytes),
            });
        },
        async readText(path = ".") {
            const bytes = await this.readFile(path);
            return codec.decode(bytes, false);
        },
        async writeText(path, text) {
            await this.writeFile(path, codec.encode(requireString(text, "writeText text")));
        },
        async readDir(path = ".") {
            const result = await request("readDir", path);
            return normalizeDirResult(result);
        },
        async mkdir(path) {
            await request("mkdir", path);
        },
        async remove(path) {
            await request("remove", path);
        },
        close() {
            target.removeEventListener("message", onMessage);
            for (const pendingRequest of pending.values()) {
                if (pendingRequest.timeout != null) {
                    clearTimeout(pendingRequest.timeout);
                }
                pendingRequest.reject(new Error("worker storage adapter closed"));
            }
            pending.clear();
        },
    };
}

function normalizeJsValueDescriptor(descriptor) {
    if (!descriptor || typeof descriptor !== "object") {
        throw new TypeError("js-value storage descriptor must be an object");
    }
    const value = String(descriptor.value ?? "").trim();
    if (!value) {
        throw new TypeError("js-value storage descriptor must include a non-empty value");
    }
    return {
        backend: "js-value",
        value,
        path: normalizeDescriptorRoot(descriptor.path),
    };
}

function normalizeWorkerDescriptor(descriptor) {
    if (!descriptor || typeof descriptor !== "object") {
        throw new TypeError("worker storage descriptor must be an object");
    }
    const worker = String(descriptor.worker ?? "").trim();
    if (!worker) {
        throw new TypeError("worker storage descriptor must include a non-empty worker");
    }
    return {
        backend: "worker",
        worker,
        path: normalizeDescriptorRoot(descriptor.path),
    };
}

function normalizeDescriptorRoot(path) {
    if (path == null || String(path).trim() === "") {
        return ".";
    }
    return normalizeRelativePath(path);
}

function normalizeRelativePath(path) {
    if (path == null) {
        return ".";
    }
    const value = String(path).trim();
    if (!value || value === ".") {
        return ".";
    }
    if (value.startsWith("/") || value.startsWith("\\")) {
        throw new TypeError(`expected a relative path, got ${JSON.stringify(path)}`);
    }
    if (value.includes("\\")) {
        throw new TypeError(`path must use forward slashes, got ${JSON.stringify(path)}`);
    }

    const parts = value.split("/");
    const normalized = [];
    for (const part of parts) {
        if (!part || part === ".") {
            throw new TypeError(`path must be a clean relative path, got ${JSON.stringify(path)}`);
        }
        if (part === "..") {
            throw new TypeError(`path must not traverse upward, got ${JSON.stringify(path)}`);
        }
        normalized.push(part);
    }
    return normalized.join("/");
}

function splitRelativePath(path) {
    const normalized = normalizeRelativePath(path);
    return normalized === "." ? [] : normalized.split("/");
}

function joinNormalizedPath(base, path) {
    const left = normalizeRelativePath(base);
    const right = normalizeRelativePath(path);
    if (left === ".") {
        return right;
    }
    if (right === ".") {
        return left;
    }
    return `${left}/${right}`;
}

function resolveNamedHandle(name, options = {}) {
    const registryHit = lookupRegistryEntry(options.registry, name);
    if (registryHit.found) {
        return registryHit;
    }

    const globals = options.globals ?? globalThis;
    const globalHit = lookupGlobalPath(globals, name);
    if (globalHit.found) {
        return globalHit;
    }

    throw new Error(`Unsupported ${options.label || "handle"} ${JSON.stringify(name)}`);
}

function lookupRegistryEntry(registry, name) {
    if (!registry) {
        return { found: false };
    }
    if (typeof registry.get === "function") {
        const value = registry.get(name);
        if (value !== undefined || (typeof registry.has === "function" && registry.has(name))) {
            return {
                found: true,
                value,
                read() {
                    return registry.get(name);
                },
                assign(nextValue) {
                    if (typeof registry.set !== "function") {
                        throw new Error(`Registry entry ${JSON.stringify(name)} is not writable`);
                    }
                    registry.set(name, nextValue);
                },
                remove() {
                    if (typeof registry.delete === "function") {
                        registry.delete(name);
                        return;
                    }
                    if (typeof registry.set === "function") {
                        registry.set(name, undefined);
                        return;
                    }
                    throw new Error(`Registry entry ${JSON.stringify(name)} is not removable`);
                },
            };
        }
    }
    if (typeof registry === "object") {
        if (Object.prototype.hasOwnProperty.call(registry, name)) {
            return {
                found: true,
                value: registry[name],
                read() {
                    return registry[name];
                },
                assign(nextValue) {
                    registry[name] = nextValue;
                },
                remove() {
                    delete registry[name];
                },
            };
        }
    }
    return { found: false };
}

function lookupGlobalPath(globals, name) {
    if (globals == null) {
        return { found: false };
    }
    if (hasProperty(globals, name)) {
        return createPropertyHandle(globals[name], globals, name, name);
    }

    const segments = splitHandlePath(name);
    if (segments.length === 0) {
        return { found: false };
    }

    let current = globals;
    let parent = null;
    let key = null;
    let offset = 0;
    if (isGlobalAlias(segments[0])) {
        if (hasProperty(globals, segments[0])) {
            parent = globals;
            key = segments[0];
            current = globals[segments[0]];
        }
        offset = 1;
    }
    for (let index = offset; index < segments.length; index += 1) {
        const segment = segments[index];
        if (!canHaveProperties(current) || !hasProperty(current, segment)) {
            return { found: false };
        }
        parent = current;
        key = segment;
        current = current[segment];
    }

    if (offset === segments.length) {
        return {
            found: true,
            value: globals,
            read() {
                return globals;
            },
            assign() {
                throw new Error(`Global handle ${JSON.stringify(name)} is not writable`);
            },
            remove() {
                throw new Error(`Global handle ${JSON.stringify(name)} is not removable`);
            },
        };
    }

    return createPropertyHandle(current, parent, key, name);
}

function createPropertyHandle(value, parent, key, label) {
    return {
        found: true,
        value,
        read() {
            if (parent == null || key == null) {
                return value;
            }
            return parent[key];
        },
        assign(nextValue) {
            if (parent == null || key == null) {
                throw new Error(`Handle ${JSON.stringify(label)} is not writable`);
            }
            parent[key] = nextValue;
        },
        remove() {
            if (parent == null || key == null) {
                throw new Error(`Handle ${JSON.stringify(label)} is not removable`);
            }
            delete parent[key];
        },
    };
}

function splitHandlePath(name) {
    return String(name)
        .split(/[/.]/u)
        .map((segment) => segment.trim())
        .filter(Boolean);
}

function isGlobalAlias(segment) {
    return segment === "globalThis" || segment === "window" || segment === "self";
}

function resolveJsRoot(handle, path) {
    const root = {
        path,
        handle,
        segments: splitRelativePath(path),
    };
    resolveNode(root, ".", "resolve");
    return root;
}

function resolveNode(root, path, op) {
    const normalizedPath = normalizeRelativePath(path);
    return traverseSegments(
        root.handle,
        root.segments.concat(splitRelativePath(normalizedPath)),
        op,
    );
}

function resolveParent(root, path, op) {
    const normalizedPath = normalizeRelativePath(path);
    const segments = root.segments.concat(splitRelativePath(normalizedPath));
    const parentNode = traverseSegments(root.handle, segments.slice(0, -1), op);
    if (!canHaveProperties(parentNode.value)) {
        throw createNodeError(op, parentNode.displayPath, "Parent is not an object-like value");
    }
    return {
        displayPath: joinNormalizedPath(root.path, normalizedPath),
        parentNode,
        key: segments[segments.length - 1],
    };
}

function traverseSegments(handle, segments, op) {
    let current = {
        displayPath: ".",
        value: handle.read ? handle.read() : handle.value,
        assign: handle.assign,
        remove: handle.remove,
    };
    for (const segment of segments) {
        if (!canHaveProperties(current.value) || !hasProperty(current.value, segment)) {
            throw createNodeError(op, joinNormalizedPath(current.displayPath, segment), "Path does not exist");
        }
        const parentValue = current.value;
        const displayPath = joinNormalizedPath(current.displayPath, segment);
        current = {
            displayPath,
            value: parentValue[segment],
            assign(nextValue) {
                parentValue[segment] = nextValue;
            },
            remove() {
                delete parentValue[segment];
            },
        };
    }
    return current;
}

function readNodeBytes(node, codec) {
    if (isDirectoryValue(node.value)) {
        throw createNodeError("readFile", node.displayPath, "Cannot read a directory");
    }
    return codec.encode(`${stringifyValue(node.value)}\n`);
}

function describeNode(node, path) {
    const kind = isDirectoryValue(node.value) ? "dir" : "file";
    return {
        path,
        kind,
        type: kind,
        size: kind === "dir" ? 0 : DEFAULT_TEXT_ENCODER.encode(`${stringifyValue(node.value)}\n`).byteLength,
        writable: typeof node.assign === "function",
    };
}

function listDirectoryEntries(value) {
    return Object.keys(value)
        .sort()
        .map((name) => {
            const child = value[name];
            const kind = isDirectoryValue(child) ? "dir" : "file";
            return { name, kind, type: kind };
        });
}

function assignChild(parentRef, nextValue, op) {
    const parentValue = parentRef.parentNode.value;
    if (!canHaveProperties(parentValue)) {
        throw createNodeError(op, parentRef.displayPath, "Parent is not an object-like value");
    }
    parentValue[parentRef.key] = nextValue;
}

function removeSelf(root, op) {
    const node = resolveNode(root, ".", op);
    if (node.value === undefined) {
        if (typeof node.remove !== "function") {
            throw createNodeError(op, node.displayPath, "Root handle is not removable");
        }
        node.remove();
        return;
    }
    if (typeof node.assign !== "function") {
        throw createNodeError(op, node.displayPath, "Root handle is not removable");
    }
    node.assign(undefined);
}

function removeChild(parentRef, op) {
    const parentValue = parentRef.parentNode.value;
    if (!canHaveProperties(parentValue)) {
        throw createNodeError(op, parentRef.displayPath, "Parent is not an object-like value");
    }

    const child = lookupChild(parentValue, parentRef.key);
    if (!child.exists) {
        throw createNodeError(op, parentRef.displayPath, "Path does not exist");
    }
    if (child.value === undefined) {
        if (Array.isArray(parentValue) && isCanonicalArrayIndex(parentRef.key)) {
            parentValue.splice(Number(parentRef.key), 1);
            return;
        }
        delete parentValue[parentRef.key];
        return;
    }
    parentValue[parentRef.key] = undefined;
}

function lookupChild(parent, key) {
    if (!canHaveProperties(parent)) {
        return { exists: false, value: undefined };
    }
    if (!hasProperty(parent, key)) {
        return { exists: false, value: undefined };
    }
    return {
        exists: true,
        value: parent[key],
    };
}

function hasProperty(value, key) {
    if (!canHaveProperties(value)) {
        return false;
    }
    return key in value;
}

function canHaveProperties(value) {
    return (typeof value === "object" && value !== null) || typeof value === "function";
}

function isDirectoryValue(value) {
    return typeof value === "object" && value !== null && typeof value !== "function";
}

function coerceAssignedValue(existingValue, text) {
    const trimmed = text.trimEnd();
    switch (typeof existingValue) {
    case "string":
        return trimmed;
    case "number":
        return Number(trimmed);
    case "bigint":
        return BigInt(trimmed);
    case "boolean":
        return !/^(?:0|false|no|n|off)$/iu.test(trimmed);
    case "symbol":
        return Symbol.for(trimmed);
    case "undefined":
        return trimmed;
    case "function":
        throw new Error("Function values are not writable through this adapter");
    case "object":
        if (existingValue === null) {
            return trimmed;
        }
        if (existingValue instanceof String) {
            return new String(trimmed);
        }
        if (existingValue instanceof Number) {
            return new Number(Number(trimmed));
        }
        if (existingValue instanceof Boolean) {
            return new Boolean(!/^(?:0|false|no|n|off)$/iu.test(trimmed));
        }
        throw new Error("Object values are not writable through this adapter");
    default:
        return trimmed;
    }
}

function stringifyValue(value) {
    return String(value);
}

function isCanonicalArrayIndex(value) {
    if (!/^(?:0|[1-9]\d*)$/u.test(String(value))) {
        return false;
    }
    return Number(value) <= Number.MAX_SAFE_INTEGER;
}

function resolvePortLikeTarget(handle) {
    const value = typeof handle?.read === "function" ? handle.read() : (handle?.value ?? handle);
    if (value && typeof value.postMessage === "function") {
        return value;
    }
    if (value && typeof value.port === "object" && value.port) {
        return value.port;
    }
    throw new Error("Unsupported worker handle target");
}

function requireMessageTarget(target) {
    if (!target || typeof target.postMessage !== "function") {
        throw new TypeError("expected a Worker, MessagePort, or other postMessage target");
    }
    if (
        typeof target.addEventListener !== "function" ||
        typeof target.removeEventListener !== "function"
    ) {
        throw new TypeError("message target must support addEventListener/removeEventListener");
    }
    return target;
}

function unwrapMessageEvent(source) {
    if (source && typeof source === "object" && "data" in source) {
        return source.data;
    }
    return source;
}

function startMessageTarget(target) {
    if (typeof target.start === "function") {
        target.start();
    }
}

function isStorageResponseMessage(payload, messageType) {
    return (
        !!payload &&
        typeof payload === "object" &&
        payload.type === messageType &&
        payload.kind === "response"
    );
}

function normalizeWorkerResult(payload, op) {
    if (!payload || typeof payload !== "object") {
        throw new Error(`${op} response must be an object`);
    }
    if (payload.result !== undefined) {
        return payload.result;
    }
    if (payload.bytes !== undefined) {
        return { bytes: payload.bytes };
    }
    return null;
}

function normalizeStatResult(result, path) {
    if (!result || typeof result !== "object") {
        throw new Error("stat response must include an object result");
    }
    const kind = String(result.kind ?? result.type ?? "").trim();
    if (kind !== "file" && kind !== "dir") {
        throw new Error("stat response kind must be \"file\" or \"dir\"");
    }
    return {
        ...result,
        path,
        kind,
        type: kind,
    };
}

function normalizeDirResult(result) {
    if (!Array.isArray(result)) {
        throw new Error("readDir response must be an array");
    }
    return result.map((entry) => {
        if (typeof entry === "string") {
            return { name: entry, kind: "file", type: "file" };
        }
        if (!entry || typeof entry !== "object") {
            throw new Error("readDir entries must be strings or objects");
        }
        const name = String(entry.name ?? "").trim();
        if (!name) {
            throw new Error("readDir entry is missing a name");
        }
        const kind = String(entry.kind ?? entry.type ?? "file").trim();
        if (kind !== "file" && kind !== "dir") {
            throw new Error(`Unsupported readDir entry kind ${JSON.stringify(kind)}`);
        }
        return {
            ...entry,
            name,
            kind,
            type: kind,
        };
    });
}

function extractBinaryResult(result, label) {
    const value =
        result && typeof result === "object" && "bytes" in result
            ? result.bytes
            : result;
    return toUint8Array(value, label).slice();
}

function protocolError(error, op) {
    if (!error) {
        return new Error(`${op} request failed`);
    }
    if (error instanceof Error) {
        return error;
    }
    if (typeof error === "string") {
        return new Error(error);
    }
    const name = error.name ? `${error.name}: ` : "";
    const message = error.message || JSON.stringify(error);
    const failure = new Error(`${name}${message}`);
    if (error.code != null) {
        failure.code = error.code;
    }
    return failure;
}

function copyTransferableBytes(value) {
    return toUint8Array(value, "binary data").slice();
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

function createTextCodec(options) {
    const encoder = options.textEncoder || DEFAULT_TEXT_ENCODER;
    const decoder = options.textDecoder || DEFAULT_TEXT_DECODER;
    if (typeof encoder.encode !== "function") {
        throw new TypeError("textEncoder must provide an encode function");
    }
    if (typeof decoder.decode !== "function") {
        throw new TypeError("textDecoder must provide a decode function");
    }
    return {
        encode(value) {
            return encoder.encode(value);
        },
        decode(bytes, fatal) {
            if (fatal) {
                return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
            }
            return decoder.decode(bytes);
        },
    };
}

function createNodeError(op, path, message) {
    const error = new Error(`${op} ${JSON.stringify(path)}: ${message}`);
    error.path = path;
    error.operation = op;
    return error;
}

function requireString(value, label) {
    if (typeof value !== "string") {
        throw new TypeError(`${label} must be a string`);
    }
    return value;
}

function errorMessage(error) {
    return error instanceof Error ? error.message : String(error);
}
