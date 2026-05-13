const encoder = new TextEncoder();
const decoder = new TextDecoder();
const files = new Map([
    ["guest.txt", encoder.encode("worker-export-ok")],
    ["dir/item.txt", encoder.encode("nested-export-ok")],
]);
const fids = new Map();

const channel = new MessageChannel();
const vm = new URL(globalThis.location.href).searchParams.get("vm");
const message = vm ? { export: channel.port2, vm } : { export: channel.port2 };
globalThis.postMessage(message, [channel.port2]);
channel.port1.addEventListener("message", (event) => {
    if (!(event.data instanceof Uint8Array)) {
        return;
    }
    channel.port1.postMessage(handleFrame(event.data));
});
channel.port1.start();
channel.port1.postMessage("!");

function handleFrame(frame) {
    const reader = new Reader(frame);
    const size = reader.u32();
    const type = reader.u8();
    const tag = reader.u16();
    if (size !== frame.byteLength) {
        return frameFrom(7, tag, new Writer().u32(5).bytes());
    }
    const out = new Writer();
    switch (type) {
    case 100:
        out.u32(reader.u32());
        out.string(reader.string());
        return frameFrom(101, tag, out.bytes());
    case 104: {
        const fid = reader.u32();
        fids.set(fid, ".");
        out.qid(".");
        return frameFrom(105, tag, out.bytes());
    }
    case 110: {
        const fid = reader.u32();
        const newfid = reader.u32();
        const count = reader.u16();
        let current = fids.get(fid) || ".";
        const qids = [];
        for (let index = 0; index < count; index += 1) {
            const name = reader.string();
            const next = joinPath(current, name);
            if (!existsPath(next)) {
                break;
            }
            current = next;
            qids.push(current);
        }
        if (qids.length === count) {
            fids.set(newfid, current);
        }
        out.u16(qids.length);
        for (const qidPath of qids) {
            out.qid(qidPath);
        }
        return frameFrom(111, tag, out.bytes());
    }
    case 12:
        out.qid(fids.get(reader.u32()) || ".");
        out.u32(8192);
        return frameFrom(13, tag, out.bytes());
    case 14: {
        const fid = reader.u32();
        const name = reader.string();
        const path = joinPath(fids.get(fid) || ".", name);
        files.set(path, new Uint8Array());
        fids.set(fid, path);
        out.qid(path);
        out.u32(8192);
        return frameFrom(15, tag, out.bytes());
    }
    case 24:
        return getattr(tag, fids.get(reader.u32()) || ".");
    case 40:
        return readdir(tag, fids.get(reader.u32()) || ".", Number(reader.u64()), reader.u32());
    case 116:
        return read(tag, fids.get(reader.u32()) || ".", Number(reader.u64()), reader.u32());
    case 118: {
        const path = fids.get(reader.u32()) || ".";
        const offset = Number(reader.u64());
        const data = reader.countedData();
        const current = files.get(path) || new Uint8Array();
        const next = new Uint8Array(Math.max(current.byteLength, offset + data.byteLength));
        next.set(current);
        next.set(data, offset);
        files.set(path, next);
        out.u32(data.byteLength);
        return frameFrom(119, tag, out.bytes());
    }
    case 120:
        fids.delete(reader.u32());
        return frameFrom(121, tag, out.bytes());
    default:
        return frameFrom(7, tag, out.u32(58).bytes());
    }
}

function getattr(tag, path) {
    const out = new Writer();
    const stat = statPath(path);
    if (!stat) {
        return frameFrom(7, tag, out.u32(2).bytes());
    }
    out.u64(0x67f);
    out.qid(path);
    out.u32(stat.kind === "dir" ? 0o040755 : 0o100644);
    out.u32(0);
    out.u32(0);
    out.u64(1);
    out.u64(0);
    out.u64(stat.size);
    out.u64(4096);
    out.u64(1);
    for (let index = 0; index < 10; index += 1) {
        out.u64(0);
    }
    return frameFrom(25, tag, out.bytes());
}

function read(tag, path, offset, count) {
    const data = files.get(path);
    if (!data) {
        return frameFrom(7, tag, new Writer().u32(2).bytes());
    }
    return frameFrom(117, tag, new Writer().countedData(data.slice(offset, offset + count)).bytes());
}

function readdir(tag, path, offset, count) {
    const entries = childrenOf(path);
    const out = new Writer();
    const body = new Writer();
    for (let index = offset; index < entries.length; index += 1) {
        const entry = entries[index];
        body.qid(joinPath(path, entry.name));
        body.u64(index + 1);
        body.u8(entry.kind === "dir" ? 4 : 8);
        body.string(entry.name);
    }
    const bytes = body.bytes().slice(0, count);
    out.countedData(bytes);
    return frameFrom(41, tag, out.bytes());
}

function statPath(path) {
    if (path === "." || isDirectory(path)) {
        return { kind: "dir", size: 0 };
    }
    const file = files.get(path);
    return file ? { kind: "file", size: file.byteLength } : null;
}

function existsPath(path) {
    return path === "." || files.has(path) || isDirectory(path);
}

function isDirectory(path) {
    const prefix = path === "." ? "" : `${path}/`;
    for (const file of files.keys()) {
        if (file.startsWith(prefix) && file.slice(prefix.length).includes("/")) {
            return true;
        }
    }
    return false;
}

function childrenOf(path) {
    const prefix = path === "." ? "" : `${path}/`;
    const entries = new Map();
    for (const file of files.keys()) {
        if (!file.startsWith(prefix)) {
            continue;
        }
        const rest = file.slice(prefix.length);
        const [name, ...tail] = rest.split("/");
        entries.set(name, { name, kind: tail.length > 0 ? "dir" : "file" });
    }
    return Array.from(entries.values()).sort((left, right) => left.name.localeCompare(right.name));
}

function joinPath(parent, child) {
    return parent === "." ? child : `${parent}/${child}`;
}

function frameFrom(type, tag, payload = new Uint8Array()) {
    const frame = new Uint8Array(7 + payload.byteLength);
    frame[0] = frame.byteLength & 0xff;
    frame[1] = (frame.byteLength >> 8) & 0xff;
    frame[2] = (frame.byteLength >> 16) & 0xff;
    frame[3] = (frame.byteLength >> 24) & 0xff;
    frame[4] = type;
    frame[5] = tag & 0xff;
    frame[6] = (tag >> 8) & 0xff;
    frame.set(payload, 7);
    return frame;
}

class Writer {
    constructor() {
        this.out = [];
    }

    u8(value) {
        this.out.push(value & 0xff);
        return this;
    }

    u16(value) {
        this.out.push(value & 0xff, (value >> 8) & 0xff);
        return this;
    }

    u32(value) {
        this.out.push(value & 0xff, (value >> 8) & 0xff, (value >> 16) & 0xff, (value >> 24) & 0xff);
        return this;
    }

    u64(value) {
        let bigint = BigInt(value);
        for (let index = 0; index < 8; index += 1) {
            this.out.push(Number((bigint >> BigInt(index * 8)) & 0xffn));
        }
        return this;
    }

    string(value) {
        const bytes = encoder.encode(String(value));
        this.u16(bytes.byteLength);
        this.out.push(...bytes);
        return this;
    }

    countedData(value) {
        const bytes = value instanceof Uint8Array ? value : new Uint8Array(value);
        this.u32(bytes.byteLength);
        this.out.push(...bytes);
        return this;
    }

    qid(path) {
        this.u8(statPath(path)?.kind === "dir" ? 0x80 : 0);
        this.u32(0);
        this.u64(hashPath(path));
        return this;
    }

    bytes() {
        return Uint8Array.from(this.out);
    }
}

class Reader {
    constructor(bytes) {
        this.bytes = bytes;
        this.offset = 0;
    }

    u8() {
        return this.bytes[this.offset++];
    }

    u16() {
        const value = this.bytes[this.offset] | (this.bytes[this.offset + 1] << 8);
        this.offset += 2;
        return value;
    }

    u32() {
        const value = this.bytes[this.offset] |
            (this.bytes[this.offset + 1] << 8) |
            (this.bytes[this.offset + 2] << 16) |
            (this.bytes[this.offset + 3] << 24);
        this.offset += 4;
        return value >>> 0;
    }

    u64() {
        let value = 0n;
        for (let index = 0; index < 8; index += 1) {
            value |= BigInt(this.bytes[this.offset + index]) << BigInt(index * 8);
        }
        this.offset += 8;
        return value;
    }

    string() {
        const length = this.u16();
        const value = decoder.decode(this.bytes.slice(this.offset, this.offset + length));
        this.offset += length;
        return value;
    }

    countedData() {
        const length = this.u32();
        const value = this.bytes.slice(this.offset, this.offset + length);
        this.offset += length;
        return value;
    }
}

function hashPath(path) {
    let hash = 2166136261;
    for (const byte of encoder.encode(path)) {
        hash ^= byte;
        hash = Math.imul(hash, 16777619);
    }
    return hash >>> 0;
}
