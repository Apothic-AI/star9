import { createFileSystemAccessStorageAdapter, createOpfsStorageAdapter } from "./storage-file-system.js";
import { createCacheStorageAdapter, createDomStorageAdapter, createDownloadStorageAdapter } from "./storage-web.js";
import { createJsValueStorageAdapter, createWorkerStorageAdapter } from "./storage-js-value.js";
import { createStarFsStorageAdapter } from "./storage-starfs.js";
import { createWanixP9NamespaceMount } from "./p9-port.js";

export class AsyncMountTable {
    constructor() {
        this._mounts = [];
    }

    mount(dst, adapter, options = {}) {
        const path = normalizeMountPath(dst);
        const record = {
            path,
            adapter: requireMountAdapter(adapter),
            kind: options.kind || "mount",
            source: options.source || null,
        };
        this.unmount(path);
        this._mounts.push(record);
        this._mounts.sort((left, right) => right.path.length - left.path.length);
        return record;
    }

    unmount(dst) {
        const path = normalizeMountPath(dst);
        const removed = [];
        this._mounts = this._mounts.filter((mount) => {
            if (mount.path !== path) {
                return true;
            }
            removed.push(mount);
            return false;
        });
        for (const mount of removed) {
            const stillMounted = this._mounts.some((other) => other.adapter === mount.adapter);
            if (!stillMounted && typeof mount.adapter.close === "function") {
                mount.adapter.close();
            }
        }
        return removed.length > 0;
    }

    resolve(path) {
        const cleanPath = normalizeMountPath(path);
        for (const mount of this._mounts) {
            if (mount.path === "." || cleanPath === mount.path || cleanPath.startsWith(`${mount.path}/`)) {
                return {
                    mount,
                    adapter: mount.adapter,
                    path:
                        mount.path === "."
                            ? cleanPath
                            : cleanPath === mount.path
                                ? "."
                                : cleanPath.slice(mount.path.length + 1),
                };
            }
        }
        return null;
    }

    entries() {
        return this._mounts.map((mount) => ({ ...mount }));
    }

    close() {
        const closed = new Set();
        for (const mount of this._mounts) {
            if (!closed.has(mount.adapter) && typeof mount.adapter.close === "function") {
                mount.adapter.close();
                closed.add(mount.adapter);
            }
        }
        this._mounts = [];
    }
}

export class BrowserDebouncedSyncScheduler {
    constructor(target, options = {}) {
        this.target = requireSyncTarget(target);
        this.debounceMs = Math.max(0, Number(options.debounceMs ?? options.debounce ?? 0));
        this.clock = options.clock || globalThis;
        this.now = typeof options.now === "function" ? options.now : () => Date.now();
        this.pending = false;
        this.running = false;
        this.dueAt = null;
        this.lastSyncedAt = null;
        this.lastError = null;
        this.requests = 0;
        this._timer = null;
        this._closed = false;
    }

    request() {
        this._assertOpen();
        this.requests += 1;
        this.pending = true;
        this.dueAt = this.now() + this.debounceMs;
        this._schedule();
        return this.snapshot();
    }

    async flush() {
        this._assertOpen();
        this._clearTimer();
        if (!this.pending || this.running) {
            return this.snapshot();
        }

        this.running = true;
        try {
            await runSyncTarget(this.target);
            this.pending = false;
            this.lastError = null;
            this.lastSyncedAt = this.now();
        } catch (error) {
            this.pending = true;
            this.lastError = error instanceof Error ? error.message : String(error);
        } finally {
            this.running = false;
            this.dueAt = null;
        }
        return this.snapshot();
    }

    snapshot() {
        return {
            pending: this.pending,
            scheduled: this._timer !== null,
            running: this.running,
            dueAt: this.dueAt,
            lastSyncedAt: this.lastSyncedAt,
            lastError: this.lastError,
            requests: this.requests,
        };
    }

    close() {
        this._clearTimer();
        this._closed = true;
    }

    _schedule() {
        this._clearTimer();
        this._timer = this.clock.setTimeout(async () => {
            this._timer = null;
            await this.flush();
        }, this.debounceMs);
    }

    _clearTimer() {
        if (this._timer !== null) {
            this.clock.clearTimeout(this._timer);
            this._timer = null;
        }
    }

    _assertOpen() {
        if (this._closed) {
            throw new Error("browser sync scheduler is closed");
        }
    }
}

export async function createBrowserStorageAdapter(descriptor, options = {}) {
    switch (descriptor?.backend) {
    case "opfs":
        return createOpfsStorageAdapter(descriptor, options.opfs || options);
    case "file-system-access":
        return createFileSystemAccessStorageAdapter(descriptor, options.fileSystemAccess || options);
    case "cache":
        return createCacheStorageAdapter(descriptor, options.cache || options);
    case "js-value":
        return createJsValueStorageAdapter(descriptor, options.jsValue || options);
    case "download":
        return createDownloadStorageAdapter(descriptor, options.download || options);
    case "dom":
        return createDomStorageAdapter(descriptor, options.dom || options);
    case "worker":
        return createWorkerStorageAdapter(descriptor, options.worker || options);
    case "starfs": {
        const backingDescriptor = descriptor.storage || {
            backend: "opfs",
            root: descriptor.root ? `starfs/${descriptor.root}` : "starfs/default",
        };
        if (backingDescriptor.backend === "starfs") {
            throw new Error("StarFS storage descriptors cannot use StarFS as their own backing store");
        }
        const backingAdapter = await createBrowserStorageAdapter(backingDescriptor, options);
        return createStarFsStorageAdapter(descriptor, {
            ...(options.starfs || options),
            backingAdapter,
        });
    }
    default:
        throw new Error(`Unsupported browser storage backend ${JSON.stringify(descriptor?.backend)}`);
    }
}

export async function createP9MountFromPort(port, options = {}) {
    return createWanixP9NamespaceMount(port, options);
}

export function normalizeMountPath(path) {
    if (path == null || path === "" || path === ".") {
        return ".";
    }
    const value = String(path);
    if (value.startsWith("/") || value.includes("\\")) {
        throw new Error(`mount paths must be relative Wanix paths: ${JSON.stringify(path)}`);
    }
    const parts = [];
    for (const part of value.split("/")) {
        if (!part || part === ".") {
            continue;
        }
        if (part === "..") {
            throw new Error(`mount paths must not traverse upward: ${JSON.stringify(path)}`);
        }
        parts.push(part);
    }
    return parts.length === 0 ? "." : parts.join("/");
}

export function dirEntriesToNames(entries) {
    return Array.from(entries || [])
        .map((entry) => (typeof entry === "string" ? entry : entry?.name))
        .filter((name) => typeof name === "string" && name.length > 0)
        .sort((left, right) => left.localeCompare(right));
}

function requireMountAdapter(adapter) {
    if (!adapter || typeof adapter !== "object") {
        throw new TypeError("expected a mount adapter object");
    }
    for (const method of ["readFile", "writeFile", "readDir"]) {
        if (typeof adapter[method] !== "function") {
            throw new TypeError(`mount adapter must implement ${method}()`);
        }
    }
    return adapter;
}

function requireSyncTarget(target) {
    if (!target || typeof target !== "object") {
        throw new TypeError("expected a browser sync target object");
    }
    if (typeof target.sync === "function" || typeof target.syncFs === "function") {
        return target;
    }
    throw new TypeError("browser sync target must implement sync() or syncFs()");
}

function runSyncTarget(target) {
    if (typeof target.sync === "function") {
        return target.sync();
    }
    return target.syncFs();
}
