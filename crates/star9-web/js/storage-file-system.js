const textEncoder = new TextEncoder()
const textDecoder = new TextDecoder()

export class StorageAdapterError extends Error {
    constructor(code, message, options = {}) {
        super(message, options)
        this.name = "StorageAdapterError"
        this.code = code
        if (options.cause !== undefined) {
            this.cause = options.cause
        }
    }
}

export function supportsOpfs(options = {}) {
    const navigatorLike = resolveNavigator(options)
    return typeof navigatorLike?.storage?.getDirectory === "function"
}

export function supportsFileSystemAccess(options = {}) {
    const scope = resolveGlobalScope(options)
    return (
        typeof scope?.showDirectoryPicker === "function" ||
        typeof scope?.showOpenFilePicker === "function" ||
        typeof scope?.showSaveFilePicker === "function" ||
        typeof scope?.FileSystemHandle === "function"
    )
}

export function detectStorageCapabilities(options = {}) {
    return {
        opfs: supportsOpfs(options),
        fileSystemAccess: supportsFileSystemAccess(options),
    }
}

export function createOpfsStorageDescriptor(root) {
    const normalizedRoot = normalizeDescriptorRoot(root)
    return normalizedRoot === undefined
        ? { backend: "opfs" }
        : { backend: "opfs", root: normalizedRoot }
}

export function createFileSystemAccessStorageDescriptor(handle, options = {}) {
    const handleId = requireNonEmptyString(handle, "file-system-access handle")
    const normalizedPath = normalizeDescriptorRoot(options.path)
    return {
        backend: "file-system-access",
        handle: handleId,
        writable: Boolean(options.writable),
        ...(normalizedPath === undefined ? {} : { path: normalizedPath }),
    }
}

export async function requestFileSystemAccessHandle(options = {}) {
    const scope = resolveGlobalScope(options)
    const mode = options.mode || "directory"
    const pickerOptions = options.pickerOptions || options
    let handle
    if (mode === "directory") {
        if (typeof scope?.showDirectoryPicker !== "function") {
            throw unsupported("File System Access directory picker is not available in this host")
        }
        handle = await scope.showDirectoryPicker(pickerOptions)
    } else if (mode === "open-file" || mode === "file") {
        if (typeof scope?.showOpenFilePicker !== "function") {
            throw unsupported("File System Access open-file picker is not available in this host")
        }
        const handles = await scope.showOpenFilePicker(pickerOptions)
        handle = Array.isArray(handles) ? handles[0] : handles
    } else if (mode === "save-file" || mode === "save") {
        if (typeof scope?.showSaveFilePicker !== "function") {
            throw unsupported("File System Access save-file picker is not available in this host")
        }
        handle = await scope.showSaveFilePicker(pickerOptions)
    } else {
        throw invalidArgument(`Unsupported File System Access picker mode ${JSON.stringify(mode)}`)
    }
    if (!handle) {
        throw storageError("ENOENT", "File System Access picker returned no handle")
    }
    const permissionMode = options.writable ? "readwrite" : "read"
    if (typeof handle.requestPermission === "function") {
        const permission = await handle.requestPermission({ mode: permissionMode })
        if (permission !== "granted") {
            throw storageError("EACCES", `File System Access permission denied: ${permission}`)
        }
    }
    return handle
}

export async function createOpfsStorageAdapter(descriptor = {}, options = {}) {
    if (descriptor?.backend && descriptor.backend !== "opfs") {
        throw invalidArgument(
            `Expected an OPFS descriptor, got backend ${JSON.stringify(descriptor.backend)}`,
        )
    }

    const rootHandle = await resolveOpfsRootHandle(options)
    const adapterRoot = await resolveAdapterRootHandle(rootHandle, descriptor.root, {
        createRoot: true,
        label: "OPFS",
    })
    return createHandleAdapter(adapterRoot, {
        writable: true,
        label: "OPFS",
    })
}

export async function createFileSystemAccessStorageAdapter(descriptor, options = {}) {
    if (!descriptor || descriptor.backend !== "file-system-access") {
        throw invalidArgument("Expected a file-system-access descriptor")
    }

    const rootHandle = await resolveFileSystemAccessHandle(descriptor, options)
    const adapterRoot = await resolveAdapterRootHandle(rootHandle, descriptor.path, {
        createRoot: true,
        label: `File System Access handle ${JSON.stringify(descriptor.handle)}`,
    })
    return createHandleAdapter(adapterRoot, {
        writable: Boolean(descriptor.writable),
        label: `File System Access handle ${JSON.stringify(descriptor.handle)}`,
    })
}

export async function createStorageAdapter(descriptor, options = {}) {
    switch (descriptor?.backend) {
    case "opfs":
        return createOpfsStorageAdapter(descriptor, options)
    case "file-system-access":
        return createFileSystemAccessStorageAdapter(descriptor, options)
    default:
        throw invalidArgument(
            `Unsupported browser storage backend ${JSON.stringify(descriptor?.backend)}`,
        )
    }
}

export function normalizeStoragePath(path) {
    return normalizePath(path)
}

function createHandleAdapter(rootHandle, options) {
    const rootKind = getHandleKind(rootHandle)
    const writable = Boolean(options.writable)
    const label = options.label || "storage adapter"

    return {
        async stat(path = ".") {
            const normalizedPath = normalizePath(path)
            const handle = await resolveHandle(rootHandle, normalizedPath)
            return createStatRecord(normalizedPath, handle)
        },

        async readFile(path) {
            const normalizedPath = normalizePath(path)
            const handle = await resolveHandle(rootHandle, normalizedPath, { expect: "file" })
            return readHandleBytes(handle, normalizedPath)
        },

        async writeFile(path, bytes) {
            ensureWritable(writable, label)
            const normalizedPath = normalizePath(path)
            const buffer = toUint8Array(bytes)
            const handle = await resolveWriteFileHandle(rootHandle, normalizedPath)
            await writeHandleBytes(handle, buffer, normalizedPath)
        },

        async readText(path) {
            const bytes = await this.readFile(path)
            return textDecoder.decode(bytes)
        },

        async writeText(path, text) {
            await this.writeFile(path, textEncoder.encode(String(text)))
        },

        async readDir(path = ".") {
            const normalizedPath = normalizePath(path)
            const handle = await resolveHandle(rootHandle, normalizedPath, { expect: "directory" })
            const children = await listDirectoryChildren(handle)
            const entries = await Promise.all(
                children
                    .sort((left, right) => left.name.localeCompare(right.name))
                    .map(async ({ name, handle: childHandle }) =>
                        createStatRecord(joinRelativePath(normalizedPath, name), childHandle, name),
                    ),
            )
            return entries
        },

        async mkdir(path) {
            ensureWritable(writable, label)
            const normalizedPath = normalizePath(path)
            if (normalizedPath === ".") {
                throw storageError("EEXIST", `${label} root already exists`)
            }
            if (rootKind !== "directory") {
                throw storageError("ENOTDIR", `${label} root is not a directory`)
            }
            const { parentHandle, name } = await resolveParentDirectory(rootHandle, normalizedPath)
            await assertMissingEntry(parentHandle, name, normalizedPath)
            await getDirectoryHandle(parentHandle, name, { create: true }, normalizedPath)
        },

        async remove(path) {
            ensureWritable(writable, label)
            const normalizedPath = normalizePath(path)
            if (normalizedPath === ".") {
                throw storageError("EINVAL", `Refusing to remove the ${label} root`)
            }
            if (rootKind !== "directory") {
                throw storageError("ENOTDIR", `${label} root is not a directory`)
            }
            const { parentHandle, name } = await resolveParentDirectory(rootHandle, normalizedPath)
            await removeDirectoryEntry(parentHandle, name, normalizedPath)
        },
    }
}

async function resolveOpfsRootHandle(options) {
    if (options.rootHandle) {
        return options.rootHandle
    }
    if (typeof options.getRootDirectory === "function") {
        return options.getRootDirectory()
    }

    const navigatorLike = resolveNavigator(options)
    if (typeof navigatorLike?.storage?.getDirectory !== "function") {
        throw unsupported("Origin Private File System API is not available in this host")
    }

    return navigatorLike.storage.getDirectory()
}

async function resolveFileSystemAccessHandle(descriptor, options) {
    if (options.rootHandle) {
        return options.rootHandle
    }
    if (typeof options.resolveHandle === "function") {
        const resolved = await options.resolveHandle(descriptor.handle, descriptor, options)
        if (resolved !== undefined && resolved !== null) {
            return resolved
        }
    }

    const handleId = requireNonEmptyString(descriptor.handle, "file-system-access handle")
    const registry = options.registry ?? options.handles
    const handle = resolveRegistryHandle(registry, handleId)
    if (handle === undefined) {
        throw storageError(
            "ENOENT",
            `File System Access handle ${JSON.stringify(handleId)} was not found in the provided registry`,
        )
    }
    return handle
}

function resolveRegistryHandle(registry, handleId) {
    if (!registry) {
        return undefined
    }
    if (typeof registry.get === "function") {
        return registry.get(handleId)
    }
    return registry[handleId]
}

async function resolveAdapterRootHandle(rootHandle, rootPath, options) {
    const normalizedRoot = normalizeDescriptorRoot(rootPath) ?? "."
    if (normalizedRoot === ".") {
        return rootHandle
    }

    if (getHandleKind(rootHandle) !== "directory") {
        throw storageError(
            "ENOTDIR",
            `${options.label} cannot be rooted at ${JSON.stringify(normalizedRoot)} because the provided handle is not a directory`,
        )
    }

    return walkDirectory(rootHandle, normalizedRoot, {
        create: Boolean(options.createRoot),
        pathLabel: normalizedRoot,
    })
}

function normalizeDescriptorRoot(root) {
    if (root === undefined || root === null) {
        return undefined
    }
    return normalizePath(root)
}

function normalizePath(path) {
    if (path === undefined || path === null || path === "") {
        return "."
    }

    if (typeof path !== "string") {
        throw invalidArgument(`Expected a path string, got ${typeof path}`)
    }
    if (path.startsWith("/")) {
        throw invalidArgument(`Absolute paths are not allowed: ${JSON.stringify(path)}`)
    }
    if (path.includes("\\")) {
        throw invalidArgument(`Backslashes are not allowed in Star9 paths: ${JSON.stringify(path)}`)
    }

    const parts = []
    for (const part of path.split("/")) {
        if (!part || part === ".") {
            continue
        }
        if (part === "..") {
            throw invalidArgument(`Path traversal is not allowed: ${JSON.stringify(path)}`)
        }
        parts.push(part)
    }

    return parts.length === 0 ? "." : parts.join("/")
}

async function resolveHandle(rootHandle, path, options = {}) {
    if (path === ".") {
        if (options.expect && getHandleKind(rootHandle) !== options.expect) {
            throw wrongKindError(path, rootHandle, options.expect)
        }
        return rootHandle
    }

    if (getHandleKind(rootHandle) !== "directory") {
        throw storageError(
            "ENOTDIR",
            `Cannot resolve ${JSON.stringify(path)} because the adapter root is not a directory`,
        )
    }

    const parentPath = parentOf(path)
    const parentHandle =
        parentPath === "."
            ? rootHandle
            : await walkDirectory(rootHandle, parentPath, { create: false, pathLabel: parentPath })
    return lookupChildHandle(parentHandle, baseName(path), path, options.expect)
}

async function resolveWriteFileHandle(rootHandle, path) {
    if (path === ".") {
        if (getHandleKind(rootHandle) !== "file") {
            throw storageError("EISDIR", "Cannot write file bytes to a directory root")
        }
        return rootHandle
    }

    if (getHandleKind(rootHandle) !== "directory") {
        throw storageError("ENOTDIR", "Cannot create nested files under a file root")
    }

    const { parentHandle, name } = await resolveParentDirectory(rootHandle, path)
    return getFileHandle(parentHandle, name, { create: true }, path)
}

async function resolveParentDirectory(rootHandle, path) {
    if (getHandleKind(rootHandle) !== "directory") {
        throw storageError("ENOTDIR", "Adapter root is not a directory")
    }

    const parentPath = parentOf(path)
    const parentHandle =
        parentPath === "."
            ? rootHandle
            : await walkDirectory(rootHandle, parentPath, { create: false, pathLabel: parentPath })
    return {
        parentHandle,
        name: baseName(path),
    }
}

async function walkDirectory(rootHandle, path, options) {
    let current = rootHandle
    for (const part of splitPath(path)) {
        current = await getDirectoryHandle(
            current,
            part,
            { create: Boolean(options.create) },
            options.pathLabel || path,
        )
    }
    return current
}

async function lookupChildHandle(directoryHandle, name, path, expect) {
    if (expect === "file") {
        return getFileHandle(directoryHandle, name, { create: false }, path)
    }
    if (expect === "directory") {
        return getDirectoryHandle(directoryHandle, name, { create: false }, path)
    }

    let fileError
    try {
        const fileHandle = await getFileHandle(directoryHandle, name, { create: false }, path)
        return fileHandle
    } catch (error) {
        fileError = error
        if (!isAlternateLookupError(error)) {
            throw error
        }
    }

    try {
        return await getDirectoryHandle(directoryHandle, name, { create: false }, path)
    } catch (error) {
        if (!isAlternateLookupError(error)) {
            throw error
        }
        throw fileError || error
    }
}

function isAlternateLookupError(error) {
    return error?.code === "ENOENT" || error?.code === "EINVAL"
}

async function assertMissingEntry(directoryHandle, name, path) {
    try {
        await lookupChildHandle(directoryHandle, name, path)
    } catch (error) {
        if (error?.code === "ENOENT") {
            return
        }
        throw error
    }
    throw storageError("EEXIST", `Path already exists: ${JSON.stringify(path)}`)
}

async function createStatRecord(path, handle, nameOverride) {
    const kind = getHandleKind(handle)
    const name = nameOverride || handle?.name || (path === "." ? "." : baseName(path))

    if (kind === "directory") {
        return {
            path,
            name,
            kind: "directory",
            type: "directory",
            size: 0,
            lastModified: null,
        }
    }

    const file = await getFileObject(handle, path)
    return {
        path,
        name,
        kind: "file",
        type: "file",
        size: Number(file?.size ?? 0),
        lastModified: file?.lastModified ?? null,
    }
}

async function listDirectoryChildren(directoryHandle) {
    if (typeof directoryHandle?.values === "function") {
        const result = []
        for await (const handle of directoryHandle.values()) {
            result.push({
                name: handle?.name ?? "",
                handle,
            })
        }
        return result
    }

    if (typeof directoryHandle?.entries === "function") {
        const result = []
        for await (const [name, handle] of directoryHandle.entries()) {
            result.push({ name, handle })
        }
        return result
    }

    if (typeof directoryHandle?.[Symbol.asyncIterator] === "function") {
        const result = []
        for await (const item of directoryHandle) {
            if (Array.isArray(item)) {
                const [name, handle] = item
                result.push({ name, handle })
            } else {
                result.push({
                    name: item?.name ?? "",
                    handle: item,
                })
            }
        }
        return result
    }

    throw unsupported("Directory handle does not support async iteration")
}

async function readHandleBytes(fileHandle, path) {
    const file = await getFileObject(fileHandle, path)
    return new Uint8Array(await file.arrayBuffer())
}

async function writeHandleBytes(fileHandle, bytes, path) {
    if (typeof fileHandle?.createWritable === "function") {
        const writable = await fileHandle.createWritable({ keepExistingData: false })
        try {
            await writable.write(bytes)
        } finally {
            if (typeof writable?.close === "function") {
                await writable.close()
            }
        }
        return
    }

    if (typeof fileHandle?.write === "function") {
        await fileHandle.write(bytes)
        return
    }

    throw unsupported(`File handle for ${JSON.stringify(path)} is not writable`)
}

async function removeDirectoryEntry(parentHandle, name, path) {
    if (typeof parentHandle?.removeEntry !== "function") {
        throw unsupported(`Directory handle for ${JSON.stringify(parentOf(path))} cannot remove entries`)
    }
    try {
        await parentHandle.removeEntry(name)
    } catch (error) {
        throw mapHandleError("remove", path, error)
    }
}

async function getDirectoryHandle(parentHandle, name, options, path) {
    if (typeof parentHandle?.getDirectoryHandle !== "function") {
        throw unsupported(`Directory handle for ${JSON.stringify(parentOf(path))} cannot resolve directories`)
    }
    try {
        return await parentHandle.getDirectoryHandle(name, options)
    } catch (error) {
        throw mapHandleError(options.create ? "mkdir" : "stat", path, error)
    }
}

async function getFileHandle(parentHandle, name, options, path) {
    if (typeof parentHandle?.getFileHandle !== "function") {
        throw unsupported(`Directory handle for ${JSON.stringify(parentOf(path))} cannot resolve files`)
    }
    try {
        return await parentHandle.getFileHandle(name, options)
    } catch (error) {
        throw mapHandleError(options.create ? "write" : "stat", path, error)
    }
}

async function getFileObject(fileHandle, path) {
    if (typeof fileHandle?.getFile !== "function") {
        throw unsupported(`File handle for ${JSON.stringify(path)} does not expose getFile()`)
    }
    try {
        return await fileHandle.getFile()
    } catch (error) {
        throw mapHandleError("read", path, error)
    }
}

function getHandleKind(handle) {
    const kind = handle?.kind
    if (kind === "file" || kind === "directory") {
        return kind
    }
    if (typeof handle?.getFile === "function" && typeof handle?.getDirectoryHandle !== "function") {
        return "file"
    }
    if (typeof handle?.getDirectoryHandle === "function") {
        return "directory"
    }
    throw unsupported("Provided handle is not a recognizable File System Access handle")
}

function toUint8Array(bytes) {
    if (bytes instanceof Uint8Array) {
        return bytes
    }
    if (ArrayBuffer.isView(bytes)) {
        return new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    }
    if (bytes instanceof ArrayBuffer) {
        return new Uint8Array(bytes)
    }
    if (Array.isArray(bytes)) {
        return Uint8Array.from(bytes)
    }
    throw invalidArgument("Expected file bytes as Uint8Array, ArrayBuffer, TypedArray, or number[]")
}

function ensureWritable(writable, label) {
    if (!writable) {
        throw storageError("EACCES", `${label} is not writable`)
    }
}

function resolveGlobalScope(options) {
    if (options.globals) {
        return options.globals
    }
    if (options.globalThis) {
        return options.globalThis
    }
    if (typeof globalThis !== "undefined") {
        return globalThis
    }
    return undefined
}

function resolveNavigator(options) {
    if (options.navigator) {
        return options.navigator
    }
    return resolveGlobalScope(options)?.navigator
}

function splitPath(path) {
    return path === "." ? [] : path.split("/")
}

function baseName(path) {
    return path === "." ? "." : path.slice(path.lastIndexOf("/") + 1)
}

function parentOf(path) {
    if (path === ".") {
        return "."
    }
    const index = path.lastIndexOf("/")
    return index === -1 ? "." : path.slice(0, index)
}

function joinRelativePath(parent, child) {
    return parent === "." ? child : `${parent}/${child}`
}

function requireNonEmptyString(value, label) {
    if (typeof value !== "string" || value.trim() === "") {
        throw invalidArgument(`${label} must not be empty`)
    }
    return value
}

function wrongKindError(path, handle, expectedKind) {
    return storageError(
        expectedKind === "directory" ? "ENOTDIR" : "EISDIR",
        `Expected ${JSON.stringify(path)} to be a ${expectedKind}, got ${getHandleKind(handle)}`,
    )
}

function mapHandleError(op, path, error) {
    if (error instanceof StorageAdapterError) {
        return error
    }

    const name = error?.name
    if (name === "NotFoundError") {
        return storageError("ENOENT", `Path does not exist: ${JSON.stringify(path)}`, error)
    }
    if (name === "TypeMismatchError") {
        return storageError("EINVAL", `Path type mismatch at ${JSON.stringify(path)}`, error)
    }
    if (name === "NotAllowedError" || name === "SecurityError") {
        return storageError("EACCES", `Access denied for ${JSON.stringify(path)}`, error)
    }
    if (name === "InvalidModificationError") {
        return storageError(
            "ENOTEMPTY",
            `Directory is not empty or cannot be modified: ${JSON.stringify(path)}`,
            error,
        )
    }

    return storageError(
        "EIO",
        `File System Access ${op} failed for ${JSON.stringify(path)}: ${error?.message || String(error)}`,
        error,
    )
}

function unsupported(message) {
    return new StorageAdapterError("ENOTSUP", message)
}

function invalidArgument(message) {
    return new StorageAdapterError("EINVAL", message)
}

function storageError(code, message, cause) {
    return new StorageAdapterError(code, message, cause === undefined ? {} : { cause })
}
