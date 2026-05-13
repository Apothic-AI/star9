import test from "node:test";
import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

const audit = JSON.parse(
    await readFile(new URL("../docs/audits/completion-gap-matrix.json", import.meta.url), "utf8"),
);

test("completion gap matrix classifies all WASI preview1 imports", () => {
    assert.deepEqual(audit.wasi_preview1.missing, []);
    for (const required of [
        "clock_res_get",
        "clock_time_get",
        "path_open",
        "fd_read",
        "fd_write",
        "sock_recv",
        "sock_send",
        "sock_shutdown",
    ]) {
        assert.ok(audit.wasi_preview1.implemented.includes(required), required);
    }
});

test("completion gap matrix classifies source unsupported markers", async () => {
    const classifications = audit.source_classifications;
    assert.ok(Array.isArray(classifications) && classifications.length > 0);
    const sourceFiles = await filesUnder(new URL("../crates", import.meta.url));
    const marker = /\b(TODO|FIXME|placeholder|not supported|unsupported|todo!|unimplemented!)\b/i;
    const unclassified = [];

    for (const file of sourceFiles) {
        if (!/\.(rs|js)$/.test(file)) {
            continue;
        }
        const text = await readFile(file, "utf8");
        const rel = filePathRelativeToRepo(file);
        text.split(/\r?\n/).forEach((line, index) => {
            if (!marker.test(line)) {
                return;
            }
            const classified = classifications.some(
                (entry) => entry.path === rel && line.includes(entry.match),
            );
            if (!classified) {
                unclassified.push(`${rel}:${index + 1}:${line.trim()}`);
            }
        });
    }

    assert.deepEqual(unclassified, []);
});

async function filesUnder(rootUrl) {
    const rootPath = rootUrl.pathname;
    const out = [];
    for (const entry of await readdir(rootPath, { withFileTypes: true })) {
        const path = join(rootPath, entry.name);
        if (entry.isDirectory()) {
            out.push(...await filesUnder(new URL(`file://${path}/`)));
        } else if (entry.isFile()) {
            out.push(path);
        }
    }
    return out;
}

function filePathRelativeToRepo(path) {
    const repo = new URL("../", import.meta.url).pathname;
    return path.startsWith(repo) ? path.slice(repo.length) : path;
}
