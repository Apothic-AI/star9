const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

const STARFS_DIR = ".starfs";
const INTERNAL_DIR = "__starfs";
const FS_DIR = "fs";
const KV_DIR = `${INTERNAL_DIR}/kv`;
const TOOLCALL_DIR = `${INTERNAL_DIR}/toolcalls`;
const SNAPSHOT_DIR = `${INTERNAL_DIR}/snapshots`;

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

export class StarFsStorageAdapter {
    constructor(backing, descriptor = {}) {
        this.backing = backing;
        this.descriptor = {
            id: String(descriptor.id || descriptor.root || "default"),
            storage: descriptor.storage || null,
            version: "wanix-starfs-adapter-v1",
        };
    }

    async initialize() {
        await ensureDir(this.backing, FS_DIR);
        await ensureDir(this.backing, INTERNAL_DIR);
        await ensureDir(this.backing, KV_DIR);
        await ensureDir(this.backing, TOOLCALL_DIR);
        await ensureDir(this.backing, SNAPSHOT_DIR);
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
        const manifest = {
            id,
            created_at: new Date().toISOString(),
            files: await collectFiles(this.backing, FS_DIR, "."),
        };
        await this.writeText(`${STARFS_DIR}/snapshots/${id}.json`, JSON.stringify(manifest));
        return manifest;
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
