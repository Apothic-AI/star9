import test from "node:test";
import assert from "node:assert/strict";

import { createBrowserNetworkDevice } from "../crates/star9-web/js/network-adapter.js";

test("browser network adapter exposes WebSocket transport through net-style files", () => {
    const sockets = [];
    class FakeWebSocket {
        constructor(url) {
            this.url = url;
            this.readyState = 0;
            this.sent = [];
            this.binaryType = "";
            this._listeners = new Map();
            sockets.push(this);
        }

        addEventListener(type, listener) {
            if (!this._listeners.has(type)) {
                this._listeners.set(type, new Set());
            }
            this._listeners.get(type).add(listener);
        }

        send(data) {
            this.sent.push(data);
        }

        close() {
            this.readyState = 3;
            this.emit("close", {});
        }

        emit(type, event) {
            for (const listener of this._listeners.get(type) ?? []) {
                listener(event);
            }
        }
    }

    const device = createBrowserNetworkDevice({ WebSocket: FakeWebSocket });
    const id = new TextDecoder().decode(device.readFile("new")).trim();
    assert.deepEqual(device.readDir("."), ["new", "1"]);
    assert.deepEqual(device.readDir(`${id}`), ["ctl", "data", "id", "local", "remote", "status"]);

    device.writeFile(`${id}/ctl`, `dial ws://example.test/socket\n`);
    assert.equal(sockets.length, 1);
    assert.equal(sockets[0].url, "ws://example.test/socket");
    assert.match(new TextDecoder().decode(device.readFile(`${id}/status`)), /^connecting remote=/);

    sockets[0].readyState = 1;
    sockets[0].emit("open", {});
    assert.match(new TextDecoder().decode(device.readFile(`${id}/status`)), /^connected local=browser remote=/);

    assert.equal(device.writeFile(`${id}/data`, new Uint8Array([1, 2, 3])), 3);
    assert.deepEqual(sockets[0].sent[0], new Uint8Array([1, 2, 3]));

    sockets[0].emit("message", { data: new Uint8Array([9, 8]) });
    assert.deepEqual(device.readFile(`${id}/data`), new Uint8Array([9, 8]));
    assert.deepEqual(device.readFile(`${id}/data`), new Uint8Array());

    assert.throws(() => device.writeFile(`${id}/ctl`, "listen tcp!0\n"), /browser raw listen is unavailable/);
    device.writeFile(`${id}/ctl`, "hangup\n");
    assert.match(new TextDecoder().decode(device.readFile(`${id}/status`)), /^closed/);
});
