import test from "node:test";
import assert from "node:assert/strict";

import {
    createFileSystemAccessStorageAdapter,
    createOpfsStorageAdapter,
    requestFileSystemAccessHandle,
} from "../crates/star9-web/js/storage-file-system.js";
import {
    createCacheStorageAdapter,
    createDomStorageAdapter,
    createDownloadStorageAdapter,
} from "../crates/star9-web/js/storage-web.js";
import {
    createJsValueStorageAdapter,
    createWorkerStorageAdapter,
} from "../crates/star9-web/js/storage-js-value.js";
import { createBrowserStorageAdapter } from "../crates/star9-web/js/mounts.js";

const encoder = new TextEncoder();

test("OPFS and File System Access adapters use safe handle-backed paths", async () => {
    const opfsRoot = new FakeDirectoryHandle("root");
    const opfs = await createOpfsStorageAdapter(
        { backend: "opfs", root: "workspace" },
        { getRootDirectory: async () => opfsRoot },
    );

    await opfs.mkdir("docs");
    await opfs.writeText("docs/readme.txt", "opfs-ok");
    assert.equal(await opfs.readText("docs/readme.txt"), "opfs-ok");
    assert.deepEqual(
        (await opfs.readDir("docs")).map((entry) => entry.name),
        ["readme.txt"],
    );
    await assert.rejects(() => opfs.stat("../escape"), /traversal|relative path/i);

    const fsaRoot = new FakeDirectoryHandle("picked");
    const fsa = await createFileSystemAccessStorageAdapter(
        { backend: "file-system-access", handle: "project", path: "data", writable: true },
        { registry: new Map([["project", fsaRoot]]) },
    );
    await fsa.writeText("state.txt", "fsa-ok");
    assert.equal(await fsa.readText("state.txt"), "fsa-ok");

    const requested = [];
    const grantedHandle = await requestFileSystemAccessHandle({
        mode: "directory",
        writable: true,
        globals: {
            showDirectoryPicker: async () => ({
                kind: "directory",
                name: "picked",
                async requestPermission(permission) {
                    requested.push(permission);
                    return "granted";
                },
            }),
        },
    });
    assert.equal(grantedHandle.name, "picked");
    assert.deepEqual(requested, [{ mode: "readwrite" }]);
});

test("Cache, DOM, and download adapters operate with fake browser hosts", async () => {
    const caches = new FakeCacheStorage();
    const cache = await createCacheStorageAdapter(
        { backend: "cache", cache: "shell", path: "root" },
        {
            caches,
            Response: FakeResponse,
            baseUrl: "https://example.invalid/cache/",
        },
    );
    await cache.writeText("asset.txt", "cache-ok");
    assert.equal(await cache.readText("asset.txt"), "cache-ok");
    assert.deepEqual(
        (await cache.readDir(".")).map((entry) => entry.name),
        ["asset.txt"],
    );

    const node = new FakeDomNode();
    const dom = await createDomStorageAdapter(
        { backend: "dom", node: "#status" },
        { resolveNode: () => node },
    );
    await dom.writeText("textContent", "dom-ok");
    await dom.writeText("attributes/title", "dom-title");
    assert.equal(await dom.readText("textContent"), "dom-ok");
    assert.equal(await dom.readText("attributes/title"), "dom-title");

    const downloaded = [];
    const download = await createDownloadStorageAdapter(
        { backend: "download", name: "artifact.txt", media_type: "text/plain" },
        {
            downloadSink: async (record) => downloaded.push(record),
            autoFlush: false,
        },
    );
    await download.writeText("out.txt", "download-ok");
    assert.equal(await download.readText("out.txt"), "download-ok");
    await download.flush("out.txt");
    assert.equal(downloaded[0].name, "artifact.txt");
    assert.equal(new TextDecoder().decode(downloaded[0].bytes), "download-ok");
});

test("JS value and worker adapters expose live host-backed storage", async () => {
    const globals = {
        app: {
            state: {
                count: 1,
                flag: true,
                nested: { value: "child" },
            },
        },
    };
    const jsValue = await createJsValueStorageAdapter(
        { backend: "js-value", value: "app", path: "state" },
        { globals },
    );

    assert.equal(await jsValue.readText("count"), "1\n");
    await jsValue.writeText("flag", "false\n");
    assert.equal(globals.app.state.flag, false);
    assert.deepEqual(
        (await jsValue.readDir(".")).map((entry) => entry.name),
        ["count", "flag", "nested"],
    );
    await jsValue.remove("nested/value");
    assert.equal(globals.app.state.nested.value, undefined);
    await jsValue.remove("nested/value");
    assert.equal(Object.hasOwn(globals.app.state.nested, "value"), false);

    const port = new FakeWorkerStoragePort();
    const worker = await createWorkerStorageAdapter(
        { backend: "worker", worker: "storage-worker" },
        { registry: new Map([["storage-worker", port]]) },
    );

    assert.equal(await worker.readText("hello.txt"), "from-worker");
    await worker.writeText("new.txt", "stored");
    assert.deepEqual(
        (await worker.readDir(".")).map((entry) => entry.name),
        ["hello.txt", "new.txt"],
    );
    worker.close();
});

test("StarFS storage is an additional mount backend beside raw OPFS", async () => {
    const root = new FakeDirectoryHandle("root");
    const options = { getRootDirectory: async () => root };
    const opfs = await createBrowserStorageAdapter(
        { backend: "opfs", root: "raw" },
        options,
    );
    const starfs = await createBrowserStorageAdapter(
        {
            backend: "starfs",
            id: "agent-a",
            storage: { backend: "opfs", root: "starfs/agent-a" },
        },
        options,
    );

    await opfs.writeText("state.txt", "raw-opfs");
    await starfs.writeText("report.txt", "starfs-file");
    await starfs.setXattr("report.txt", "user.mime", encoder.encode("text/plain"));
    await starfs.setKv("prefs", { theme: "dark" });
    const snapshot = await starfs.createSnapshot("initial");
    await starfs.writeText("report.txt", "changed");
    await starfs.restoreSnapshot("initial");

    assert.equal(await opfs.readText("state.txt"), "raw-opfs");
    assert.equal(await starfs.readText("report.txt"), "starfs-file");
    assert.deepEqual(await starfs.listXattrs("report.txt"), ["user.mime"]);
    assert.equal(new TextDecoder().decode(await starfs.getXattr("report.txt", "user.mime")), "text/plain");
    assert.deepEqual(await starfs.getKv("prefs"), { theme: "dark" });
    assert.deepEqual(snapshot.files, [{ path: "report.txt", size: 11 }]);
    assert.deepEqual(await starfs.listSnapshots(), ["initial"]);
    assert.deepEqual(
        (await starfs.readDir(".")).map((entry) => entry.name),
        [".starfs", "report.txt"],
    );
    await assert.rejects(() => starfs.link("report.txt", "copy.txt"), /hard links|ENOTSUP/i);
    await assert.rejects(() => opfs.readText("report.txt"), /Path does not exist|not found/i);
});

test("StarFS SDK storage is a separate optional backend", async () => {
    const sdk = new FakeStarFsSdkAdapter();
    const starfs = await createBrowserStorageAdapter(
        { backend: "starfs-sdk", id: "sdk-workspace" },
        {
            starfsSdk: {
                factory: async (descriptor) => {
                    assert.equal(descriptor.backend, "starfs-sdk");
                    assert.equal(descriptor.id, "sdk-workspace");
                    return sdk;
                },
            },
        },
    );

    await starfs.writeText("notes.txt", "from-sdk");
    await starfs.setXattr("notes.txt", "user.kind", encoder.encode("note"));
    assert.equal(await starfs.readText("notes.txt"), "from-sdk");
    assert.deepEqual(await starfs.listXattrs("notes.txt"), ["user.kind"]);
    assert.equal(new TextDecoder().decode(await starfs.getXattr("notes.txt", "user.kind")), "note");
    assert.deepEqual((await starfs.readDir(".")).map((entry) => entry.name), ["notes.txt"]);
    assert.equal(starfs.descriptor.version, "star9-starfs-sdk-adapter-v1");
});

class FakeStarFsSdkAdapter {
    constructor() {
        this.files = new Map();
        this.xattrs = new Map();
    }

    async stat(path = ".") {
        if (path === ".") {
            return { name: ".", kind: "dir", type: "dir", size: 0 };
        }
        const bytes = this.files.get(path);
        if (!bytes) {
            throw Object.assign(new Error(`not found: ${path}`), { code: "ENOENT" });
        }
        return { name: path.split("/").pop(), kind: "file", type: "file", size: bytes.byteLength };
    }

    async readFile(path) {
        const bytes = this.files.get(path);
        if (!bytes) {
            throw Object.assign(new Error(`not found: ${path}`), { code: "ENOENT" });
        }
        return new Uint8Array(bytes);
    }

    async writeFile(path, bytes) {
        this.files.set(path, new Uint8Array(bytes));
    }

    async readDir(path = ".") {
        if (path !== ".") {
            throw Object.assign(new Error(`not found: ${path}`), { code: "ENOENT" });
        }
        return [...this.files.keys()].sort().map((name) => ({
            name,
            kind: "file",
            type: "file",
            size: this.files.get(name).byteLength,
        }));
    }

    async mkdir() {}

    async remove(path) {
        this.files.delete(path);
    }

    async setXattr(path, name, value) {
        await this.stat(path);
        this.xattrs.set(`${path}\0${name}`, new Uint8Array(value));
    }

    async getXattr(path, name) {
        const value = this.xattrs.get(`${path}\0${name}`);
        if (!value) {
            throw Object.assign(new Error(`xattr not found: ${name}`), { code: "ENOENT" });
        }
        return new Uint8Array(value);
    }

    async listXattrs(path) {
        await this.stat(path);
        const prefix = `${path}\0`;
        return [...this.xattrs.keys()]
            .filter((key) => key.startsWith(prefix))
            .map((key) => key.slice(prefix.length))
            .sort();
    }

    async removeXattr(path, name) {
        this.xattrs.delete(`${path}\0${name}`);
    }
}

class FakeFileHandle {
    constructor(name, bytes = new Uint8Array()) {
        this.kind = "file";
        this.name = name;
        this.bytes = new Uint8Array(bytes);
    }

    async getFile() {
        const bytes = new Uint8Array(this.bytes);
        return {
            size: bytes.byteLength,
            lastModified: 0,
            async arrayBuffer() {
                return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
            },
        };
    }

    async createWritable() {
        return {
            write: async (bytes) => {
                this.bytes = new Uint8Array(bytes);
            },
            close: async () => {},
        };
    }
}

class FakeDirectoryHandle {
    constructor(name) {
        this.kind = "directory";
        this.name = name;
        this.children = new Map();
    }

    async getDirectoryHandle(name, options = {}) {
        const child = this.children.get(name);
        if (child?.kind === "directory") {
            return child;
        }
        if (child) {
            throw domException("TypeMismatchError");
        }
        if (!options.create) {
            throw domException("NotFoundError");
        }
        const next = new FakeDirectoryHandle(name);
        this.children.set(name, next);
        return next;
    }

    async getFileHandle(name, options = {}) {
        const child = this.children.get(name);
        if (child?.kind === "file") {
            return child;
        }
        if (child) {
            throw domException("TypeMismatchError");
        }
        if (!options.create) {
            throw domException("NotFoundError");
        }
        const next = new FakeFileHandle(name);
        this.children.set(name, next);
        return next;
    }

    async removeEntry(name) {
        if (!this.children.delete(name)) {
            throw domException("NotFoundError");
        }
    }

    async *entries() {
        for (const entry of this.children.entries()) {
            yield entry;
        }
    }
}

class FakeCacheStorage {
    constructor() {
        this.caches = new Map();
    }

    async open(name) {
        if (!this.caches.has(name)) {
            this.caches.set(name, new FakeCache());
        }
        return this.caches.get(name);
    }
}

class FakeCache {
    constructor() {
        this.responses = new Map();
    }

    async match(url) {
        return this.responses.get(url)?.clone() ?? undefined;
    }

    async put(url, response) {
        this.responses.set(url, response.clone ? response.clone() : response);
    }

    async delete(url) {
        return this.responses.delete(url);
    }

    async keys() {
        return [...this.responses.keys()].map((url) => ({ url }));
    }
}

class FakeResponse {
    constructor(bytes, init = {}) {
        this.bytes = new Uint8Array(bytes);
        const headers = new Map(Object.entries(init.headers ?? {}));
        this.headers = {
            get: (name) => headers.get(name.toLowerCase()) ?? headers.get(name) ?? null,
        };
    }

    async arrayBuffer() {
        return this.bytes.buffer.slice(this.bytes.byteOffset, this.bytes.byteOffset + this.bytes.byteLength);
    }

    clone() {
        return new FakeResponse(this.bytes, {
            headers: { "content-length": String(this.bytes.byteLength) },
        });
    }
}

class FakeDomNode {
    constructor() {
        this.textContent = "";
        this.value = "";
        this.dataset = {};
        this.attributes = new Map();
    }

    getAttribute(name) {
        return this.attributes.get(name) ?? null;
    }

    setAttribute(name, value) {
        this.attributes.set(name, String(value));
    }

    removeAttribute(name) {
        this.attributes.delete(name);
    }

    hasAttribute(name) {
        return this.attributes.has(name);
    }

    getAttributeNames() {
        return [...this.attributes.keys()];
    }
}

class FakeWorkerStoragePort extends EventTarget {
    constructor() {
        super();
        this.files = new Map([["hello.txt", encoder.encode("from-worker")]]);
    }

    start() {}

    postMessage(message) {
        queueMicrotask(() => {
            this.dispatchEvent(messageEvent({
                type: message.type,
                kind: "response",
                id: message.id,
                ok: true,
                result: this.handle(message),
            }));
        });
    }

    handle(message) {
        switch (message.op) {
        case "stat":
            return this.files.has(message.path)
                ? { name: message.path, kind: "file", size: this.files.get(message.path).byteLength }
                : { name: ".", kind: "dir", size: 0 };
        case "readFile":
            return { bytes: this.files.get(message.path) ?? new Uint8Array() };
        case "writeFile":
            this.files.set(message.path, new Uint8Array(message.bytes));
            return null;
        case "readDir":
            return [...this.files.keys()].sort().map((name) => ({ name, kind: "file" }));
        case "mkdir":
        case "remove":
            return null;
        default:
            throw new Error(`unexpected op ${message.op}`);
        }
    }
}

function messageEvent(data) {
    const event = new Event("message");
    Object.defineProperty(event, "data", { value: data });
    return event;
}

function domException(name) {
    const error = new Error(name);
    error.name = name;
    return error;
}
