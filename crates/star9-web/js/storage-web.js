const DEFAULT_DOWNLOAD_MEDIA_TYPE = "application/octet-stream";
const DEFAULT_BASE_URL = "https://star9.invalid/";

export function detectStorageWebCapabilities(options = {}) {
    return {
        cache: supportsCacheStorage(options),
        dom: supportsDomStorage(options),
        download: supportsDownloadStorage(options),
    };
}

export function supportsCacheStorage(options = {}) {
    const globals = getGlobals(options);
    const cacheApi = options.caches || globals.caches;
    return Boolean(cacheApi && typeof cacheApi.open === "function");
}

export function supportsDomStorage(options = {}) {
    if (typeof options.resolveNode === "function") {
        return true;
    }
    const document = options.document || getGlobals(options).document;
    return Boolean(document && typeof document.querySelector === "function");
}

export function supportsDownloadStorage(options = {}) {
    if (typeof options.downloadSink === "function") {
        return true;
    }

    const globals = getGlobals(options);
    const document = options.document || globals.document;
    const BlobCtor = options.Blob || globals.Blob;
    const urlApi = options.URL || globals.URL;

    return Boolean(
        document &&
        typeof document.createElement === "function" &&
        BlobCtor &&
        urlApi &&
        typeof urlApi.createObjectURL === "function" &&
        typeof urlApi.revokeObjectURL === "function",
    );
}

export async function createCacheStorageAdapter(descriptor, options = {}) {
    if (!supportsCacheStorage(options)) {
        throw new Error("Cache storage adapter requires a host with the Cache API");
    }

    const globals = getGlobals(options);
    const cacheApi = options.caches || globals.caches;
    const ResponseCtor = options.Response || globals.Response;
    if (typeof ResponseCtor !== "function") {
        throw new Error("Cache storage adapter requires the Response constructor");
    }

    const cacheName = requireNonEmptyString(descriptor?.cache, "cache name");
    const rootPath = normalizeRelativePath(descriptor?.path ?? ".", { label: "cache path" });
    const baseUrl = normalizeDirectoryUrl(
        options.baseUrl || globals.document?.baseURI || globals.location?.href || DEFAULT_BASE_URL,
        globals.document?.baseURI || globals.location?.href || DEFAULT_BASE_URL,
    );
    const cache = await cacheApi.open(cacheName);
    const virtualDirs = new Set(["."]);

    return {
        kind: "cache",
        descriptor: shallowCloneDescriptor(descriptor),
        capabilities: detectStorageWebCapabilities(options),
        cacheName,
        baseUrl,
        rootPath,
        stat,
        readFile,
        writeFile,
        readText,
        writeText,
        readDir,
        mkdir,
        remove,
    };

    async function stat(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath === ".") {
            return createDirStat(".");
        }

        const response = await matchFile(relativePath);
        if (response) {
            return createFileStat(relativePath, await responseSize(response));
        }

        if (await isDirectory(relativePath)) {
            return createDirStat(relativePath);
        }

        throw notFoundError("cache path", relativePath);
    }

    async function readFile(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath === ".") {
            throw new Error("Cannot read cache directory '.' as a file");
        }

        const response = await matchFile(relativePath);
        if (!response) {
            throw notFoundError("cache file", relativePath);
        }

        return responseBytes(response);
    }

    async function writeFile(path, bytes) {
        const relativePath = normalizeWritablePath(path, "cache file");
        const data = toUint8Array(bytes);
        const headers = { "content-length": String(data.byteLength) };
        const response = new ResponseCtor(data, {
            status: 200,
            statusText: "OK",
            headers,
        });

        await cache.put(toRequestUrl(relativePath), response);
        return createFileStat(relativePath, data.byteLength);
    }

    async function readText(path) {
        const bytes = await readFile(path);
        return decodeUtf8(bytes, options);
    }

    async function writeText(path, text) {
        return writeFile(path, encodeUtf8(String(text), options));
    }

    async function readDir(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath !== "." && await matchFile(relativePath)) {
            throw new Error(`Cache path ${JSON.stringify(relativePath)} is a file, not a directory`);
        }

        const files = await listFiles();
        const entries = buildDirectoryEntries(relativePath, files, virtualDirs);
        if (!entries && relativePath !== ".") {
            throw notFoundError("cache directory", relativePath);
        }
        return entries || [];
    }

    async function mkdir(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath !== "." && await matchFile(relativePath)) {
            throw new Error(`Cannot create cache directory ${JSON.stringify(relativePath)} because a file already exists there`);
        }

        addDirectoryAndParents(virtualDirs, relativePath);
        return createDirStat(relativePath);
    }

    async function remove(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath === ".") {
            throw new Error("Cannot remove the cache adapter root directory '.'");
        }

        const response = await matchFile(relativePath);
        if (response) {
            const deleted = await cache.delete(toRequestUrl(relativePath));
            if (!deleted) {
                throw notFoundError("cache file", relativePath);
            }
            return;
        }

        const files = await listFiles();
        const matchingFiles = files.filter((filePath) => isDescendantPath(relativePath, filePath));
        const hadVirtualDir = hasDirectory(relativePath, files, virtualDirs);

        if (matchingFiles.length === 0 && !hadVirtualDir) {
            throw notFoundError("cache path", relativePath);
        }

        for (const filePath of matchingFiles) {
            await cache.delete(toRequestUrl(filePath));
        }

        removeDirectoryAndDescendants(virtualDirs, relativePath);
    }

    async function matchFile(relativePath) {
        const response = await cache.match(toRequestUrl(relativePath));
        return response || null;
    }

    function toRequestUrl(relativePath) {
        const fullPath = joinRelativePath(rootPath, relativePath);
        return new URL(encodePathSegments(fullPath), baseUrl).href;
    }

    async function listFiles() {
        const requests = await cache.keys();
        const paths = [];

        for (const request of requests) {
            const relativePath = requestUrlToRelativePath(request.url, baseUrl, rootPath);
            if (relativePath) {
                paths.push(relativePath);
            }
        }

        return uniqueSorted(paths);
    }

    async function isDirectory(relativePath) {
        const files = await listFiles();
        return hasDirectory(relativePath, files, virtualDirs);
    }
}

export async function createDomStorageAdapter(descriptor, options = {}) {
    if (!supportsDomStorage(options)) {
        throw new Error("DOM storage adapter requires a host document or a custom resolveNode option");
    }

    const globals = getGlobals(options);
    const document = options.document || globals.document;
    const selector = requireNonEmptyString(descriptor?.node, "dom node selector");
    const defaultTarget = descriptor?.property
        ? parseDomTargetSelector(descriptor.property)
        : null;
    const resolveNode = options.resolveNode || ((nodeSelector) => document.querySelector(nodeSelector));

    return {
        kind: "dom",
        descriptor: shallowCloneDescriptor(descriptor),
        capabilities: detectStorageWebCapabilities(options),
        selector,
        defaultProperty: descriptor?.property ?? null,
        stat,
        readFile,
        writeFile,
        readText,
        writeText,
        readDir,
        mkdir,
        remove,
    };

    function stat(path) {
        const relativePath = normalizeStoragePath(path);
        const node = getNode();
        const target = resolveTarget(relativePath);

        if (target.kind === "root" || target.kind === "dir") {
            return createDirStat(relativePath);
        }

        if (!domTargetExists(node, target)) {
            throw notFoundError("dom path", relativePath);
        }

        return createFileStat(relativePath, encodeUtf8(readDomTarget(node, target), options).byteLength);
    }

    function readFile(path) {
        return encodeUtf8(readText(path), options);
    }

    function writeFile(path, bytes) {
        return writeText(path, decodeUtf8(toUint8Array(bytes), options));
    }

    function readText(path) {
        const relativePath = normalizeStoragePath(path);
        const node = getNode();
        const target = resolveTarget(relativePath);

        if (target.kind === "root" || target.kind === "dir") {
            throw new Error(`Cannot read DOM directory ${JSON.stringify(relativePath)} as a file`);
        }
        if (!domTargetExists(node, target)) {
            throw notFoundError("dom path", relativePath);
        }

        return readDomTarget(node, target);
    }

    function writeText(path, text) {
        const relativePath = normalizeWritablePath(path, "dom path");
        const node = getNode();
        const target = resolveTarget(relativePath);

        if (target.kind === "dir") {
            throw new Error(`Cannot write text to DOM directory ${JSON.stringify(relativePath)}`);
        }

        writeDomTarget(node, target, String(text));
        return stat(relativePath);
    }

    function readDir(path) {
        const relativePath = normalizeStoragePath(path);
        const node = getNode();
        const target = resolveTarget(relativePath);

        if (target.kind === "file") {
            throw new Error(`DOM path ${JSON.stringify(relativePath)} is a file, not a directory`);
        }
        if (target.kind === "dir" && target.name === "property") {
            return [];
        }

        if (target.kind === "root") {
            if (defaultTarget) {
                return [createDirEntry("value", "file")];
            }
            return [
                createDirEntry("textContent", "file"),
                createDirEntry("value", "file"),
                createDirEntry("property", "dir"),
                createDirEntry("attributes", "dir"),
                createDirEntry("dataset", "dir"),
            ];
        }

        if (target.name === "attributes") {
            return listDomAttributeEntries(node);
        }

        if (target.name === "dataset") {
            return listDomDatasetEntries(node);
        }

        return [];
    }

    function mkdir(path) {
        const relativePath = normalizeStoragePath(path);
        const target = resolveTarget(relativePath);

        if (target.kind === "file") {
            throw new Error(`Cannot create DOM directory ${JSON.stringify(relativePath)} because it is a file path`);
        }
        return createDirStat(relativePath);
    }

    function remove(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath === ".") {
            throw new Error("Cannot remove the DOM adapter root directory '.'");
        }

        const node = getNode();
        const target = resolveTarget(relativePath);
        if (target.kind !== "file") {
            throw new Error(`Cannot remove DOM directory ${JSON.stringify(relativePath)}`);
        }
        clearDomTarget(node, target);
    }

    function getNode() {
        const node = resolveNode(selector, {
            descriptor,
            document,
            globals,
            options,
        });
        if (!node) {
            throw new Error(`DOM storage adapter could not resolve node ${JSON.stringify(selector)}`);
        }
        return node;
    }

    function resolveTarget(relativePath) {
        if (relativePath === ".") {
            return { kind: "root" };
        }

        if (defaultTarget) {
            if (relativePath === "value") {
                return defaultTarget;
            }
            throw notFoundError("dom path", relativePath);
        }

        if (relativePath === "textContent" || relativePath === "value") {
            return { kind: "file", target: { type: "property", key: relativePath } };
        }
        if (relativePath === "property" || relativePath === "attributes" || relativePath === "dataset") {
            return { kind: "dir", name: relativePath };
        }

        const parts = relativePath.split("/");
        if (parts.length === 2 && parts[0] === "property") {
            return { kind: "file", target: { type: "property", key: parts[1] } };
        }
        if (parts.length === 2 && parts[0] === "attributes") {
            return { kind: "file", target: { type: "attribute", key: parts[1] } };
        }
        if (parts.length === 2 && parts[0] === "dataset") {
            return { kind: "file", target: { type: "dataset", key: parts[1] } };
        }

        throw notFoundError("dom path", relativePath);
    }
}

export async function createDownloadStorageAdapter(descriptor, options = {}) {
    if (!supportsDownloadStorage(options)) {
        throw new Error("Download storage adapter requires a download sink or browser download APIs");
    }

    const downloadSink = options.downloadSink || createAnchorDownloadSink(options);
    const defaultName = descriptor?.name ? validateDownloadName(descriptor.name) : null;
    const mediaType = descriptor?.media_type || DEFAULT_DOWNLOAD_MEDIA_TYPE;
    const autoFlush = options.autoFlush !== false;
    const retainAfterFlush = options.retainAfterFlush === true;
    const pending = new Map();
    const virtualDirs = new Set(["."]);
    const flushed = [];

    return {
        kind: "download",
        descriptor: shallowCloneDescriptor(descriptor),
        capabilities: detectStorageWebCapabilities(options),
        stat,
        readFile,
        writeFile,
        readText,
        writeText,
        readDir,
        mkdir,
        remove,
        flush,
        flushAll,
        listDownloads,
        pendingEntries,
    };

    function stat(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath === ".") {
            return createDirStat(".");
        }

        const data = pending.get(relativePath);
        if (data) {
            return createFileStat(relativePath, data.byteLength);
        }

        if (hasDirectory(relativePath, [...pending.keys()], virtualDirs)) {
            return createDirStat(relativePath);
        }

        throw notFoundError("download path", relativePath);
    }

    function readFile(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath === ".") {
            throw new Error("Cannot read download directory '.' as a file");
        }

        const data = pending.get(relativePath);
        if (!data) {
            throw notFoundError("download file", relativePath);
        }

        return new Uint8Array(data);
    }

    async function writeFile(path, bytes) {
        const relativePath = normalizeWritablePath(path, "download file");
        const data = toUint8Array(bytes);
        pending.set(relativePath, new Uint8Array(data));

        if (autoFlush) {
            await flush(relativePath);
        }

        return createFileStat(relativePath, data.byteLength);
    }

    function readText(path) {
        return decodeUtf8(readFile(path), options);
    }

    async function writeText(path, text) {
        return writeFile(path, encodeUtf8(String(text), options));
    }

    function readDir(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath !== "." && pending.has(relativePath)) {
            throw new Error(`Download path ${JSON.stringify(relativePath)} is a file, not a directory`);
        }

        const entries = buildDirectoryEntries(relativePath, [...pending.keys()], virtualDirs);
        if (!entries && relativePath !== ".") {
            throw notFoundError("download directory", relativePath);
        }
        return entries || [];
    }

    function mkdir(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath !== "." && pending.has(relativePath)) {
            throw new Error(`Cannot create download directory ${JSON.stringify(relativePath)} because a file already exists there`);
        }

        addDirectoryAndParents(virtualDirs, relativePath);
        return createDirStat(relativePath);
    }

    function remove(path) {
        const relativePath = normalizeStoragePath(path);
        if (relativePath === ".") {
            throw new Error("Cannot remove the download adapter root directory '.'");
        }

        if (pending.delete(relativePath)) {
            return;
        }

        const descendants = [...pending.keys()].filter((filePath) => isDescendantPath(relativePath, filePath));
        const hadVirtualDir = hasDirectory(relativePath, [...pending.keys()], virtualDirs);
        if (descendants.length === 0 && !hadVirtualDir) {
            throw notFoundError("download path", relativePath);
        }

        for (const filePath of descendants) {
            pending.delete(filePath);
        }

        removeDirectoryAndDescendants(virtualDirs, relativePath);
    }

    async function flush(path) {
        const relativePath = normalizeWritablePath(path, "download file");
        const data = pending.get(relativePath);
        if (!data) {
            throw notFoundError("download file", relativePath);
        }

        const bytes = new Uint8Array(data);
        const name = defaultName || baseName(relativePath);
        const record = {
            path: relativePath,
            name,
            mediaType,
            bytes,
            size: bytes.byteLength,
        };

        await downloadSink(record);
        flushed.push({
            path: record.path,
            name: record.name,
            mediaType: record.mediaType,
            size: record.size,
        });

        if (!retainAfterFlush) {
            pending.delete(relativePath);
        }

        return createFileStat(relativePath, record.size);
    }

    async function flushAll() {
        const paths = [...pending.keys()].sort();
        const stats = [];
        for (const relativePath of paths) {
            stats.push(await flush(relativePath));
        }
        return stats;
    }

    function listDownloads() {
        return flushed.map((entry) => ({ ...entry }));
    }

    function pendingEntries() {
        return [...pending.entries()]
            .sort(([left], [right]) => left.localeCompare(right))
            .map(([path, data]) => ({
                path,
                size: data.byteLength,
                bytes: new Uint8Array(data),
            }));
    }
}

export function createAnchorDownloadSink(options = {}) {
    const globals = getGlobals(options);
    const document = options.document || globals.document;
    const BlobCtor = options.Blob || globals.Blob;
    const urlApi = options.URL || globals.URL;

    if (
        !document ||
        typeof document.createElement !== "function" ||
        typeof BlobCtor !== "function" ||
        !urlApi ||
        typeof urlApi.createObjectURL !== "function" ||
        typeof urlApi.revokeObjectURL !== "function"
    ) {
        throw new Error("Browser anchor downloads are not supported by the current host");
    }

    return async function anchorDownloadSink(record) {
        const blob = new BlobCtor([record.bytes], { type: record.mediaType || DEFAULT_DOWNLOAD_MEDIA_TYPE });
        const href = urlApi.createObjectURL(blob);

        try {
            const anchor = document.createElement("a");
            anchor.href = href;
            anchor.download = record.name;
            if (typeof anchor.click !== "function") {
                throw new Error("Browser download anchor does not support click()");
            }
            anchor.click();
        } finally {
            urlApi.revokeObjectURL(href);
        }
    };
}

export function normalizeStoragePath(path) {
    return normalizeRelativePath(path, { label: "path" });
}

function getGlobals(options) {
    return options.globals || globalThis;
}

function shallowCloneDescriptor(descriptor) {
    return descriptor ? { ...descriptor } : {};
}

function requireNonEmptyString(value, label) {
    if (typeof value !== "string" || value.trim() === "") {
        throw new Error(`Expected ${label} to be a non-empty string`);
    }
    return value.trim();
}

function normalizeRelativePath(path, options = {}) {
    const label = options.label || "path";
    const rawValue = path == null ? "." : String(path);

    if (rawValue === "" || rawValue === ".") {
        return ".";
    }
    if (rawValue.includes("\0")) {
        throw new Error(`${label} must not contain NUL bytes`);
    }
    if (rawValue.includes("\\")) {
        throw new Error(`${label} must use forward slashes`);
    }
    if (rawValue.startsWith("/") || /^[A-Za-z]:\//.test(rawValue)) {
        throw new Error(`${label} must be a relative path`);
    }

    const parts = rawValue.split("/");
    if (parts.some((part) => part === "" || part === "." || part === "..")) {
        throw new Error(`${label} must be a clean relative path`);
    }
    if (parts.some((part) => part.includes("?") || part.includes("#"))) {
        throw new Error(`${label} must not contain query or fragment characters`);
    }

    return parts.join("/");
}

function normalizeWritablePath(path, label) {
    const normalized = normalizeRelativePath(path, { label });
    if (normalized === ".") {
        throw new Error(`Cannot write to ${label} '.'`);
    }
    return normalized;
}

function joinRelativePath(basePath, relativePath) {
    const cleanBasePath = normalizeRelativePath(basePath, { label: "base path" });
    const cleanRelativePath = normalizeRelativePath(relativePath, { label: "relative path" });

    if (cleanBasePath === ".") {
        return cleanRelativePath;
    }
    if (cleanRelativePath === ".") {
        return cleanBasePath;
    }
    return `${cleanBasePath}/${cleanRelativePath}`;
}

function normalizeDirectoryUrl(value, reference) {
    const url = new URL(String(value), reference);
    if (!url.pathname.endsWith("/")) {
        url.pathname = `${url.pathname}/`;
    }
    url.search = "";
    url.hash = "";
    return url.href;
}

function encodePathSegments(path) {
    const normalized = normalizeRelativePath(path, { label: "cache path" });
    if (normalized === ".") {
        return "";
    }
    return normalized.split("/").map((segment) => encodeURIComponent(segment)).join("/");
}

function requestUrlToRelativePath(requestUrl, baseUrl, rootPath) {
    const base = new URL(baseUrl);
    const request = new URL(requestUrl, base);

    if (
        request.protocol !== base.protocol ||
        request.username !== base.username ||
        request.password !== base.password ||
        request.host !== base.host
    ) {
        return null;
    }
    if (!request.pathname.startsWith(base.pathname)) {
        return null;
    }
    if (request.search || request.hash) {
        return null;
    }

    const encodedRelativePath = request.pathname.slice(base.pathname.length);
    if (!encodedRelativePath) {
        return null;
    }

    const fullPath = decodePathSegments(encodedRelativePath);
    const cleanFullPath = normalizeRelativePath(fullPath, { label: "cache request path" });
    const cleanRootPath = normalizeRelativePath(rootPath, { label: "cache root path" });

    if (cleanRootPath === ".") {
        return cleanFullPath;
    }
    if (cleanFullPath === cleanRootPath) {
        return null;
    }
    if (!cleanFullPath.startsWith(`${cleanRootPath}/`)) {
        return null;
    }

    return cleanFullPath.slice(cleanRootPath.length + 1);
}

function decodePathSegments(encodedPath) {
    return encodedPath
        .split("/")
        .filter(Boolean)
        .map((segment) => decodeURIComponent(segment))
        .join("/");
}

function toUint8Array(value) {
    if (value instanceof Uint8Array) {
        return new Uint8Array(value);
    }
    if (value instanceof ArrayBuffer) {
        return new Uint8Array(value.slice(0));
    }
    if (ArrayBuffer.isView(value)) {
        return new Uint8Array(value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength));
    }
    if (Array.isArray(value)) {
        return Uint8Array.from(value);
    }
    throw new TypeError("Expected bytes to be a Uint8Array, ArrayBuffer, ArrayBufferView, or number[]");
}

function encodeUtf8(text, options = {}) {
    const globals = getGlobals(options);
    const Encoder = globals.TextEncoder || TextEncoder;
    return new Encoder().encode(String(text));
}

function decodeUtf8(bytes, options = {}) {
    const globals = getGlobals(options);
    const Decoder = globals.TextDecoder || TextDecoder;
    return new Decoder("utf-8").decode(bytes);
}

async function responseBytes(response) {
    return new Uint8Array(await response.arrayBuffer());
}

async function responseSize(response) {
    const headerValue = response.headers?.get?.("content-length");
    if (headerValue && /^\d+$/.test(headerValue)) {
        return Number(headerValue);
    }
    return (await responseBytes(response)).byteLength;
}

function createDirStat(path) {
    return {
        name: baseName(path),
        path,
        kind: "dir",
        type: "dir",
        size: 0,
    };
}

function createFileStat(path, size) {
    return {
        name: baseName(path),
        path,
        kind: "file",
        type: "file",
        size,
    };
}

function createDirEntry(path, kind) {
    return {
        name: baseName(path),
        path,
        kind,
        type: kind,
    };
}

function baseName(path) {
    const normalized = normalizeRelativePath(path, { label: "path" });
    if (normalized === ".") {
        return ".";
    }
    const parts = normalized.split("/");
    return parts[parts.length - 1];
}

function parentPath(path) {
    const normalized = normalizeRelativePath(path, { label: "path" });
    if (normalized === "." || !normalized.includes("/")) {
        return ".";
    }
    return normalized.slice(0, normalized.lastIndexOf("/"));
}

function addParentDirectories(directories, filePath) {
    let cursor = parentPath(filePath);
    while (true) {
        directories.add(cursor);
        if (cursor === ".") {
            return;
        }
        cursor = parentPath(cursor);
    }
}

function addDirectoryAndParents(directories, directoryPath) {
    let cursor = normalizeRelativePath(directoryPath, { label: "directory path" });
    while (true) {
        directories.add(cursor);
        if (cursor === ".") {
            return;
        }
        cursor = parentPath(cursor);
    }
}

function removeDirectoryAndDescendants(directories, directoryPath) {
    const normalized = normalizeRelativePath(directoryPath, { label: "directory path" });
    for (const entry of [...directories]) {
        if (entry === normalized || isDescendantPath(normalized, entry)) {
            directories.delete(entry);
        }
    }
    directories.add(".");
}

function pruneVirtualDirectories(directories, files) {
    const keep = new Set(["."]);

    for (const filePath of files) {
        addParentDirectories(keep, filePath);
    }

    for (const entry of [...directories]) {
        if (!keep.has(entry)) {
            directories.delete(entry);
        }
    }
}

function hasDirectory(directoryPath, filePaths, directories) {
    const normalized = normalizeRelativePath(directoryPath, { label: "directory path" });
    if (normalized === ".") {
        return true;
    }
    if (directories.has(normalized)) {
        return true;
    }
    return filePaths.some((filePath) => isDescendantPath(normalized, filePath));
}

function isDescendantPath(parentPathValue, childPathValue) {
    const parent = normalizeRelativePath(parentPathValue, { label: "parent path" });
    const child = normalizeRelativePath(childPathValue, { label: "child path" });
    return child.startsWith(`${parent}/`);
}

function buildDirectoryEntries(directoryPath, filePaths, directories) {
    const normalized = normalizeRelativePath(directoryPath, { label: "directory path" });
    if (!hasDirectory(normalized, filePaths, directories)) {
        return null;
    }

    const entries = new Map();
    for (const filePath of filePaths) {
        const remainder = childRemainder(normalized, filePath);
        if (remainder == null || remainder === "") {
            continue;
        }

        const [name, ...rest] = remainder.split("/");
        const entryPath = normalized === "." ? name : `${normalized}/${name}`;
        const kind = rest.length > 0 ? "dir" : "file";
        if (entries.get(name) === "dir" || kind === "dir") {
            entries.set(name, "dir");
        } else {
            entries.set(name, "file");
        }
        if (!entries.has(`${name}:path`)) {
            entries.set(`${name}:path`, entryPath);
        }
    }

    for (const directoryEntry of directories) {
        const remainder = childRemainder(normalized, directoryEntry);
        if (remainder == null || remainder === "") {
            continue;
        }

        const [name] = remainder.split("/");
        const entryPath = normalized === "." ? name : `${normalized}/${name}`;
        entries.set(name, "dir");
        if (!entries.has(`${name}:path`)) {
            entries.set(`${name}:path`, entryPath);
        }
    }

    return [...entries.keys()]
        .filter((key) => !key.endsWith(":path"))
        .sort()
        .map((name) => createDirEntry(entries.get(`${name}:path`), entries.get(name)));
}

function childRemainder(parent, child) {
    if (parent === ".") {
        if (child === ".") {
            return "";
        }
        return child;
    }
    if (child === parent) {
        return "";
    }
    if (!child.startsWith(`${parent}/`)) {
        return null;
    }
    return child.slice(parent.length + 1);
}

function uniqueSorted(values) {
    return [...new Set(values)].sort();
}

function parseDomTargetSelector(selector) {
    const value = requireNonEmptyString(selector, "dom property");

    if (
        value.startsWith("attributes.") ||
        value.startsWith("attribute.") ||
        value.startsWith("attributes/") ||
        value.startsWith("attribute/")
    ) {
        return {
            kind: "file",
            target: {
                type: "attribute",
                key: value.slice(value.search(/[./]/) + 1),
            },
        };
    }
    if (value.startsWith("dataset.") || value.startsWith("dataset/")) {
        return {
            kind: "file",
            target: {
                type: "dataset",
                key: value.slice(value.search(/[./]/) + 1),
            },
        };
    }
    if (value.startsWith("property.") || value.startsWith("property/")) {
        return {
            kind: "file",
            target: {
                type: "property",
                key: value.slice(value.search(/[./]/) + 1),
            },
        };
    }

    return {
        kind: "file",
        target: {
            type: "property",
            key: value,
        },
    };
}

function domTargetExists(node, targetInfo) {
    const target = unwrapDomTarget(targetInfo);
    switch (target.type) {
    case "attribute":
        return node.hasAttribute(target.key);
    case "dataset":
        return getDomDatasetValue(node, target.key) != null;
    case "property":
        return target.key === "textContent" || target.key === "value" || target.key in node;
    default:
        return false;
    }
}

function readDomTarget(node, targetInfo) {
    const target = unwrapDomTarget(targetInfo);
    switch (target.type) {
    case "attribute": {
        const value = node.getAttribute(target.key);
        if (value == null) {
            throw notFoundError("dom attribute", target.key);
        }
        return value;
    }
    case "dataset": {
        const value = getDomDatasetValue(node, target.key);
        if (value == null) {
            throw notFoundError("dom dataset entry", target.key);
        }
        return value;
    }
    case "property":
        return String(node[target.key] ?? "");
    default:
        throw new Error(`Unsupported DOM target type ${JSON.stringify(target.type)}`);
    }
}

function writeDomTarget(node, targetInfo, value) {
    const target = unwrapDomTarget(targetInfo);
    switch (target.type) {
    case "attribute":
        node.setAttribute(target.key, value);
        return;
    case "dataset":
        setDomDatasetValue(node, target.key, value);
        return;
    case "property":
        node[target.key] = value;
        return;
    default:
        throw new Error(`Unsupported DOM target type ${JSON.stringify(target.type)}`);
    }
}

function clearDomTarget(node, targetInfo) {
    const target = unwrapDomTarget(targetInfo);
    switch (target.type) {
    case "attribute":
        node.removeAttribute(target.key);
        return;
    case "dataset":
        deleteDomDatasetValue(node, target.key);
        return;
    case "property":
        if (target.key === "textContent" || target.key === "value") {
            node[target.key] = "";
            return;
        }
        if (!Reflect.deleteProperty(node, target.key)) {
            node[target.key] = undefined;
        }
        return;
    default:
        throw new Error(`Unsupported DOM target type ${JSON.stringify(target.type)}`);
    }
}

function unwrapDomTarget(targetInfo) {
    if (targetInfo.kind !== "file" || !targetInfo.target) {
        throw new Error("Expected a DOM file target");
    }
    return targetInfo.target;
}

function listDomAttributeEntries(node) {
    const names = typeof node.getAttributeNames === "function"
        ? node.getAttributeNames()
        : [];
    return names
        .slice()
        .sort()
        .map((name) => createDirEntry(`attributes/${name}`, "file"));
}

function listDomDatasetEntries(node) {
    const dataset = node.dataset || {};
    return Object.keys(dataset)
        .sort()
        .map((name) => createDirEntry(`dataset/${name}`, "file"));
}

function getDomDatasetValue(node, key) {
    const datasetKey = toDatasetPropertyName(key);
    if (node.dataset && datasetKey in node.dataset) {
        return node.dataset[datasetKey];
    }
    return node.getAttribute?.(toDatasetAttributeName(key));
}

function setDomDatasetValue(node, key, value) {
    const datasetKey = toDatasetPropertyName(key);
    if (node.dataset) {
        node.dataset[datasetKey] = value;
        return;
    }
    node.setAttribute(toDatasetAttributeName(key), value);
}

function deleteDomDatasetValue(node, key) {
    const datasetKey = toDatasetPropertyName(key);
    if (node.dataset && datasetKey in node.dataset) {
        delete node.dataset[datasetKey];
        return;
    }
    node.removeAttribute?.(toDatasetAttributeName(key));
}

function toDatasetPropertyName(key) {
    return String(key).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function toDatasetAttributeName(key) {
    return `data-${String(key).replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`;
}

function validateDownloadName(name) {
    const value = requireNonEmptyString(name, "download name");
    if (value.includes("/") || value.includes("\\")) {
        throw new Error("Download name must be a single file name without path separators");
    }
    return value;
}

function notFoundError(kind, path) {
    return new Error(`No such ${kind}: ${JSON.stringify(path)}`);
}
