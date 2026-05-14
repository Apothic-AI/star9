const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

const STARFS_DIR = ".starfs";
const INTERNAL_DIR = "__starfs";
const FS_DIR = "fs";
const KV_DIR = `${INTERNAL_DIR}/kv`;
const TOOLCALL_DIR = `${INTERNAL_DIR}/toolcalls`;
const SNAPSHOT_DIR = `${INTERNAL_DIR}/snapshots`;
const XATTR_DIR = `${INTERNAL_DIR}/xattrs`;

export async function createStarFsStorageAdapter(descriptor = {}, options = {}) {
    if (descriptor?.backend && descriptor.backend !== "starfs") {
        throw new TypeError(`Expected a StarFS descriptor, got ${JSON.stringify(descriptor.backend)}`);
    }
    const backing = options.backingAdapter || options.backing;
    if (!backing || typeof backing !== "object") {
        throw new TypeError("StarFS storage requires a backing adapter");
    }
    for (const method of ["stat", "readFile", "writeFile", "readDir", "mkdir", "remove"]) {
        if (typeof backing[method] !== "function") {
            throw new TypeError(`StarFS backing adapter must implement ${method}()`);
        }
    }

    const adapter = new StarFsStorageAdapter(backing, descriptor);
    await adapter.initialize();
    return adapter;
}

export async function createStarFsSdkStorageAdapter(descriptor = {}, options = {}) {
    if (descriptor?.backend && descriptor.backend !== "starfs-sdk") {
        throw new TypeError(`Expected a StarFS SDK descriptor, got ${JSON.stringify(descriptor.backend)}`);
    }
    const factory =
        options.factory ||
        options.sdkFactory ||
        options.createClient ||
        globalThis.Star9StarFsSdk?.createAdapter ||
        globalThis.StarFS?.createAdapter;
    if (typeof factory !== "function") {
        throw new TypeError("StarFS SDK storage requires a factory/createAdapter function");
    }
    const sdk = await factory({ ...descriptor, backend: "starfs-sdk" }, options);
    return new StarFsSdkStorageAdapter(sdk, descriptor);
}

export class StarFsStorageAdapter {
    constructor(backing, descriptor = {}) {
        this.backing = backing;
        this.descriptor = {
            id: String(descriptor.id || descriptor.root || "default"),
            storage: descriptor.storage || null,
            version: "star9-starfs-adapter-v1",
        };
    }

    async initialize() {
        await ensureDir(this.backing, FS_DIR);
        await ensureDir(this.backing, INTERNAL_DIR);
        await ensureDir(this.backing, KV_DIR);
        await ensureDir(this.backing, TOOLCALL_DIR);
        await ensureDir(this.backing, SNAPSHOT_DIR);
        await ensureDir(this.backing, XATTR_DIR);
        await this.backing.writeFile(`${INTERNAL_DIR}/meta.json`, jsonBytes(this.descriptor));
    }

    async stat(path = ".") {
        const normalized = normalizePath(path);
        if (normalized === ".") {
            return dirStat(".");
        }
        if (normalized === STARFS_DIR) {
            return dirStat(STARFS_DIR);
        }
        return this.backing.stat(this._mapPath(normalized));
    }

    async readFile(path) {
        return this.backing.readFile(this._mapPath(normalizePath(path)));
    }

    async writeFile(path, bytes) {
        const normalized = normalizePath(path);
        if (normalized === "." || normalized === STARFS_DIR) {
            throw storageError("EISDIR", `Cannot write StarFS directory ${JSON.stringify(path)}`);
        }
        await ensureDir(this.backing, parentPath(this._mapPath(normalized)));
        await this.backing.writeFile(this._mapPath(normalized), toUint8Array(bytes));
    }

    async readText(path) {
        return textDecoder.decode(await this.readFile(path));
    }

    async writeText(path, text) {
        await this.writeFile(path, textEncoder.encode(String(text)));
    }

    async readDir(path = ".") {
        const normalized = normalizePath(path);
        if (normalized === ".") {
            const entries = await safeReadDir(this.backing, FS_DIR);
            return mergeEntries(entries, [dirStat(STARFS_DIR)]);
        }
        return this.backing.readDir(this._mapPath(normalized));
    }

    async mkdir(path) {
        const normalized = normalizePath(path);
        if (normalized === "." || normalized === STARFS_DIR) {
            throw storageError("EEXIST", `StarFS directory exists: ${JSON.stringify(path)}`);
        }
        await ensureDir(this.backing, parentPath(this._mapPath(normalized)));
        await this.backing.mkdir(this._mapPath(normalized));
    }

    async remove(path) {
        const normalized = normalizePath(path);
        if (normalized === "." || normalized === STARFS_DIR) {
            throw storageError("EINVAL", `Refusing to remove StarFS root ${JSON.stringify(path)}`);
        }
        await this.backing.remove(this._mapPath(normalized));
    }

    async link(oldPath, newPath) {
        if (typeof this.backing.link !== "function") {
            throw storageError("ENOTSUP", "StarFS lightweight adapter backing store does not support hard links");
        }
        await ensureDir(this.backing, parentPath(this._mapPath(normalizePath(newPath))));
        await this.backing.link(this._mapPath(normalizePath(oldPath)), this._mapPath(normalizePath(newPath)));
    }

    async setXattr(path, name, value) {
        await this.stat(path);
        const attrPath = this._xattrPath(path, name);
        await ensureDir(this.backing, parentPath(attrPath));
        await this.backing.writeFile(attrPath, toUint8Array(value));
    }

    async getXattr(path, name) {
        await this.stat(path);
        return this.backing.readFile(this._xattrPath(path, name));
    }

    async listXattrs(path) {
        await this.stat(path);
        const entries = await safeReadDir(this.backing, this._xattrPath(path));
        return entries
            .filter((entry) => kindOf(entry) !== "dir")
            .map((entry) => entry.name.replace(/\.bin$/, ""))
            .sort();
    }

    async removeXattr(path, name) {
        await this.stat(path);
        await this.backing.remove(this._xattrPath(path, name));
    }

    async setKv(key, value) {
        const normalized = normalizeKey(key);
        await this.writeText(`${STARFS_DIR}/kv/${normalized}.json`, JSON.stringify(value));
    }

    async getKv(key) {
        const normalized = normalizeKey(key);
        return JSON.parse(await this.readText(`${STARFS_DIR}/kv/${normalized}.json`));
    }

    async recordToolCall(record) {
        const now = Date.now();
        const name = normalizeKey(record?.name || "tool");
        const id = `${String(now).padStart(13, "0")}-${name}.json`;
        await this.writeText(`${STARFS_DIR}/toolcalls/${id}`, JSON.stringify({
            id,
            name,
            parameters: record?.parameters ?? null,
            result: record?.result ?? null,
            error: record?.error ?? null,
            started_at: record?.started_at ?? now / 1000,
            completed_at: record?.completed_at ?? now / 1000,
            duration_ms: record?.duration_ms ?? 0,
        }));
        return id;
    }

    async createSnapshot(name = "snapshot") {
        const id = normalizeKey(name);
        const entries = await collectSnapshotEntries(this.backing, FS_DIR, ".");
        const manifest = {
            id,
            created_at: new Date().toISOString(),
            files: entries.map(({ content_base64: _content, ...entry }) => entry),
            entries,
        };
        await this.writeText(`${STARFS_DIR}/snapshots/${id}.json`, JSON.stringify(manifest));
        return manifest;
    }

    async listSnapshots() {
        const entries = await safeReadDir(this.backing, SNAPSHOT_DIR);
        return entries
            .filter((entry) => kindOf(entry) !== "dir" && entry.name.endsWith(".json"))
            .map((entry) => entry.name.slice(0, -".json".length))
            .sort();
    }

    async restoreSnapshot(snapshot) {
        const manifest =
            typeof snapshot === "string"
                ? JSON.parse(await this.readText(`${STARFS_DIR}/snapshots/${normalizeKey(snapshot)}.json`))
                : snapshot;
        if (!manifest || !Array.isArray(manifest.entries)) {
            throw storageError("EINVAL", "StarFS snapshot manifest must include entries");
        }
        for (const entry of manifest.entries) {
            await this.writeFile(entry.path, base64ToBytes(entry.content_base64 || ""));
        }
    }

    close() {
        if (typeof this.backing.close === "function") {
            this.backing.close();
        }
    }

    _mapPath(path) {
        const normalized = normalizePath(path);
        if (normalized === ".") {
            return FS_DIR;
        }
        if (normalized === STARFS_DIR) {
            return INTERNAL_DIR;
        }
        if (normalized.startsWith(`${STARFS_DIR}/`)) {
            return `${INTERNAL_DIR}/${normalized.slice(STARFS_DIR.length + 1)}`;
        }
        return `${FS_DIR}/${normalized}`;
    }

    _xattrPath(path, name = null) {
        const dir = `${XATTR_DIR}/${pathKey(path)}`;
        if (name == null) {
            return dir;
        }
        return `${dir}/${normalizeKey(name)}.bin`;
    }
}

export class StarFsSdkStorageAdapter {
    constructor(sdk, descriptor = {}) {
        if (!sdk || typeof sdk !== "object") {
            throw new TypeError("StarFS SDK factory returned no adapter");
        }
        for (const method of ["stat", "readFile", "writeFile", "readDir", "mkdir", "remove"]) {
            if (typeof sdk[method] !== "function") {
                throw new TypeError(`StarFS SDK adapter must implement ${method}()`);
            }
        }
        this.sdk = sdk;
        this.descriptor = {
            id: String(descriptor.id || descriptor.root || "sdk"),
            storage: descriptor.storage || null,
            version: "star9-starfs-sdk-adapter-v1",
        };
    }

    stat(path = ".") {
        return this.sdk.stat(normalizePath(path));
    }

    readFile(path) {
        return this.sdk.readFile(normalizePath(path));
    }

    writeFile(path, bytes) {
        return this.sdk.writeFile(normalizePath(path), toUint8Array(bytes));
    }

    async readText(path) {
        return textDecoder.decode(await this.readFile(path));
    }

    async writeText(path, text) {
        await this.writeFile(path, textEncoder.encode(String(text)));
    }

    readDir(path = ".") {
        return this.sdk.readDir(normalizePath(path));
    }

    mkdir(path) {
        return this.sdk.mkdir(normalizePath(path));
    }

    remove(path) {
        return this.sdk.remove(normalizePath(path));
    }

    link(oldPath, newPath) {
        if (typeof this.sdk.link !== "function") {
            throw storageError("ENOTSUP", "StarFS SDK adapter does not expose link()");
        }
        return this.sdk.link(normalizePath(oldPath), normalizePath(newPath));
    }

    setXattr(path, name, value) {
        if (typeof this.sdk.setXattr !== "function") {
            throw storageError("ENOTSUP", "StarFS SDK adapter does not expose setXattr()");
        }
        return this.sdk.setXattr(normalizePath(path), String(name), toUint8Array(value));
    }

    getXattr(path, name) {
        if (typeof this.sdk.getXattr !== "function") {
            throw storageError("ENOTSUP", "StarFS SDK adapter does not expose getXattr()");
        }
        return this.sdk.getXattr(normalizePath(path), String(name));
    }

    listXattrs(path) {
        if (typeof this.sdk.listXattrs !== "function") {
            throw storageError("ENOTSUP", "StarFS SDK adapter does not expose listXattrs()");
        }
        return this.sdk.listXattrs(normalizePath(path));
    }

    removeXattr(path, name) {
        if (typeof this.sdk.removeXattr !== "function") {
            throw storageError("ENOTSUP", "StarFS SDK adapter does not expose removeXattr()");
        }
        return this.sdk.removeXattr(normalizePath(path), String(name));
    }

    createSnapshot(name) {
        if (typeof this.sdk.createSnapshot !== "function") {
            throw storageError("ENOTSUP", "StarFS SDK adapter does not expose createSnapshot()");
        }
        return this.sdk.createSnapshot(String(name || "snapshot"));
    }

    listSnapshots() {
        if (typeof this.sdk.listSnapshots !== "function") {
            throw storageError("ENOTSUP", "StarFS SDK adapter does not expose listSnapshots()");
        }
        return this.sdk.listSnapshots();
    }

    restoreSnapshot(snapshot) {
        if (typeof this.sdk.restoreSnapshot !== "function") {
            throw storageError("ENOTSUP", "StarFS SDK adapter does not expose restoreSnapshot()");
        }
        return this.sdk.restoreSnapshot(snapshot);
    }

    close() {
        if (typeof this.sdk.close === "function") {
            this.sdk.close();
        }
    }
}

async function collectFiles(backing, root, displayRoot) {
    const entries = [];
    for (const entry of await safeReadDir(backing, root)) {
        const childStoragePath = `${root}/${entry.name}`;
        const childDisplayPath = displayRoot === "." ? entry.name : `${displayRoot}/${entry.name}`;
        if (kindOf(entry) === "dir") {
            entries.push(...await collectFiles(backing, childStoragePath, childDisplayPath));
        } else {
            const bytes = await backing.readFile(childStoragePath);
            entries.push({
                path: childDisplayPath,
                size: bytes.byteLength,
            });
        }
    }
    return entries.sort((left, right) => left.path.localeCompare(right.path));
}

async function collectSnapshotEntries(backing, root, displayRoot) {
    const files = await collectFiles(backing, root, displayRoot);
    const entries = [];
    for (const file of files) {
        const bytes = await backing.readFile(`${FS_DIR}/${file.path}`);
        entries.push({
            ...file,
            content_base64: bytesToBase64(bytes),
        });
    }
    return entries;
}

async function ensureDir(adapter, path) {
    const normalized = normalizePath(path);
    if (normalized === ".") {
        return;
    }
    const parts = normalized.split("/");
    let current = ".";
    for (const part of parts) {
        current = current === "." ? part : `${current}/${part}`;
        try {
            const stat = await adapter.stat(current);
            if (kindOf(stat) !== "dir") {
                throw storageError("ENOTDIR", `Path is not a directory: ${current}`);
            }
        } catch (error) {
            if (!isMissing(error)) {
                throw error;
            }
            await adapter.mkdir(current);
        }
    }
}

async function safeReadDir(adapter, path) {
    try {
        return await adapter.readDir(path);
    } catch (error) {
        if (isMissing(error)) {
            return [];
        }
        throw error;
    }
}

function mergeEntries(left, right) {
    const entries = new Map();
    for (const entry of [...left, ...right]) {
        entries.set(entry.name, entry);
    }
    return [...entries.values()].sort((a, b) => a.name.localeCompare(b.name));
}

function dirStat(name) {
    return {
        name,
        path: name,
        kind: "dir",
        type: "dir",
        size: 0,
    };
}

function jsonBytes(value) {
    return textEncoder.encode(JSON.stringify(value, null, 2));
}

function kindOf(stat) {
    const value = stat?.kind || stat?.type;
    return value === "dir" || value === "directory" ? "dir" : "file";
}

function parentPath(path) {
    const normalized = normalizePath(path);
    if (normalized === "." || !normalized.includes("/")) {
        return ".";
    }
    return normalized.slice(0, normalized.lastIndexOf("/"));
}

function normalizeKey(value) {
    const normalized = String(value ?? "").trim().replace(/[^A-Za-z0-9._-]+/g, "-");
    if (!normalized) {
        throw storageError("EINVAL", "StarFS key must not be empty");
    }
    return normalized;
}

function pathKey(path) {
    return encodeURIComponent(normalizePath(path)).replace(/[!'()*]/g, (ch) =>
        `%${ch.charCodeAt(0).toString(16).toUpperCase()}`,
    );
}

function normalizePath(path) {
    if (path == null || path === "" || path === ".") {
        return ".";
    }
    const value = String(path);
    if (value.startsWith("/") || value.includes("\\")) {
        throw storageError("EINVAL", `Storage paths must be relative: ${JSON.stringify(path)}`);
    }
    const parts = [];
    for (const part of value.split("/")) {
        if (!part || part === ".") {
            continue;
        }
        if (part === "..") {
            throw storageError("EINVAL", `Storage paths must not traverse upward: ${JSON.stringify(path)}`);
        }
        parts.push(part);
    }
    return parts.length === 0 ? "." : parts.join("/");
}

function bytesToBase64(bytes) {
    const value = toUint8Array(bytes);
    let binary = "";
    for (let i = 0; i < value.byteLength; i += 0x8000) {
        const chunk = value.subarray(i, i + 0x8000);
        binary += String.fromCharCode(...chunk);
    }
    return btoa(binary);
}

function base64ToBytes(value) {
    const binary = atob(String(value));
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
        bytes[i] = binary.charCodeAt(i);
    }
    return bytes;
}

function isMissing(error) {
    return String(error?.code || "").toUpperCase() === "ENOENT" || /not found|does not exist/i.test(String(error?.message || error));
}

function storageError(code, message) {
    const error = new Error(message);
    error.code = code;
    return error;
}

function toUint8Array(value) {
    if (value instanceof Uint8Array) {
        return value;
    }
    if (value instanceof ArrayBuffer) {
        return new Uint8Array(value);
    }
    if (ArrayBuffer.isView(value)) {
        return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    return textEncoder.encode(String(value));
}
