import test from "node:test";
import assert from "node:assert/strict";

import {
    AsyncMountTable,
    BrowserDebouncedSyncScheduler,
} from "../crates/star9-web/js/mounts.js";

test("AsyncMountTable resolves longest mounted prefix and closes unmounted adapters", async () => {
    const root = fakeAdapter();
    const nested = fakeAdapter();
    const table = new AsyncMountTable();

    table.mount("mnt", root);
    table.mount("mnt/nested", nested);

    assert.equal(table.resolve("mnt/file.txt").adapter, root);
    assert.equal(table.resolve("mnt/file.txt").path, "file.txt");
    assert.equal(table.resolve("mnt/nested/file.txt").adapter, nested);
    assert.equal(table.resolve("mnt/nested/file.txt").path, "file.txt");

    assert.equal(table.unmount("mnt/nested"), true);
    assert.equal(nested.closed, true);
    assert.equal(table.resolve("mnt/nested/file.txt").adapter, root);

    table.close();
    assert.equal(root.closed, true);
});

test("AsyncMountTable keeps shared adapters open until their final mount is removed", async () => {
    const adapter = fakeAdapter();
    const table = new AsyncMountTable();

    table.mount("task-export", adapter);
    table.mount("vm-guest", adapter);

    assert.equal(table.unmount("task-export"), true);
    assert.equal(adapter.closed, false);
    assert.equal(table.unmount("vm-guest"), true);
    assert.equal(adapter.closed, true);

    adapter.closed = false;
    table.mount("task-export", adapter);
    table.mount("vm-guest", adapter);
    table.close();
    assert.equal(adapter.closed, true);
});

test("BrowserDebouncedSyncScheduler debounces browser timers and flushes once", async () => {
    const clock = new FakeClock();
    const calls = [];
    const scheduler = new BrowserDebouncedSyncScheduler(
        { sync: async () => calls.push(clock.now) },
        { debounceMs: 50, clock, now: () => clock.now },
    );

    scheduler.request();
    clock.advance(25);
    scheduler.request();
    assert.deepEqual(calls, []);
    assert.equal(scheduler.snapshot().pending, true);
    assert.equal(scheduler.snapshot().scheduled, true);

    await clock.advance(49);
    assert.deepEqual(calls, []);
    await clock.advance(1);

    assert.deepEqual(calls, [75]);
    assert.equal(scheduler.snapshot().pending, false);
    assert.equal(scheduler.snapshot().scheduled, false);
    assert.equal(scheduler.snapshot().lastSyncedAt, 75);
    assert.equal(scheduler.snapshot().lastError, null);
});

test("BrowserDebouncedSyncScheduler keeps failed work pending until a later request", async () => {
    const clock = new FakeClock();
    const outcomes = [new Error("sync failed"), null];
    const scheduler = new BrowserDebouncedSyncScheduler(
        {
            async syncFs() {
                const outcome = outcomes.shift();
                if (outcome) throw outcome;
            },
        },
        { debounceMs: 10, clock, now: () => clock.now },
    );

    scheduler.request();
    await clock.advance(10);
    assert.equal(scheduler.snapshot().pending, true);
    assert.equal(scheduler.snapshot().scheduled, false);
    assert.equal(scheduler.snapshot().lastError, "sync failed");

    scheduler.request();
    await clock.advance(10);
    assert.equal(scheduler.snapshot().pending, false);
    assert.equal(scheduler.snapshot().lastError, null);
    assert.equal(scheduler.snapshot().lastSyncedAt, 20);
});

function fakeAdapter() {
    return {
        closed: false,
        async readFile() {},
        async writeFile() {},
        async readDir() {},
        close() {
            this.closed = true;
        },
    };
}

class FakeClock {
    constructor() {
        this.now = 0;
        this.nextId = 1;
        this.timers = new Map();
    }

    setTimeout(callback, delay) {
        const id = this.nextId++;
        this.timers.set(id, {
            due: this.now + delay,
            callback,
        });
        return id;
    }

    clearTimeout(id) {
        this.timers.delete(id);
    }

    async advance(ms) {
        const end = this.now + ms;
        while (true) {
            const next = [...this.timers.entries()]
                .filter(([, timer]) => timer.due <= end)
                .sort((left, right) => left[1].due - right[1].due)[0];
            if (!next) break;
            const [id, timer] = next;
            this.timers.delete(id);
            this.now = timer.due;
            await timer.callback();
        }
        this.now = end;
    }
}
