import { createFileSystemAccessStorageAdapter, createOpfsStorageAdapter } from "./storage-file-system.js";
import { createCacheStorageAdapter, createDomStorageAdapter, createDownloadStorageAdapter } from "./storage-web.js";
import { createJsValueStorageAdapter, createWorkerStorageAdapter } from "./storage-js-value.js";
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
            if (typeof mount.adapter.close === "function") {
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
        for (const mount of this._mounts) {
            if (typeof mount.adapter.close === "function") {
                mount.adapter.close();
            }
        }
        this._mounts = [];
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
