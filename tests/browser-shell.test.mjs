import test from "node:test";
import assert from "node:assert/strict";

globalThis.HTMLElement ??= class HTMLElement {};

const { createStar9Shell, splitShellWords } = await import("../crates/star9-web/js/shell.js");

test("splitShellWords handles quotes and escapes for browser shell commands", () => {
    assert.deepEqual(
        splitShellWords("mount-starfs work 'agent one'"),
        ["mount-starfs", "work", "agent one"],
    );
    assert.deepEqual(
        splitShellWords('download "dir/file name.txt"'),
        ["download", "dir/file name.txt"],
    );
    assert.deepEqual(splitShellWords("write a hello\\ world"), ["write", "a", "hello world"]);
});

test("Star9ShellController defaults to the rc shell facade", async () => {
    const calls = [];
    const controller = createStar9Shell({
        system: {
            createRcShell() {
                return {
                    eval(line) {
                        calls.push(line);
                        return { status: 0, stdout: "ok\n", stderr: "" };
                    },
                    prompt() {
                        return "star9:.$ ";
                    },
                    cwd() {
                        return ".";
                    },
                    lastStatus() {
                        return 0;
                    },
                };
            },
        },
    });

    assert.equal(controller.prompt(), "star9:.$ ");
    assert.equal(controller.cwd(), ".");
    assert.equal(controller.lastStatus(), 0);
    assert.deepEqual(await controller.eval("version"), {
        status: 0,
        stdout: "ok\n",
        stderr: "",
    });
    assert.deepEqual(calls, ["version"]);
});

test("Star9ShellController can request the simple shell facade", async () => {
    let created = false;
    const controller = createStar9Shell({
        system: {
            createShell() {
                created = true;
                return {
                    eval(line) {
                        assert.equal(line, "version");
                        return { status: 0, stdout: "0.1.0\n", stderr: "" };
                    },
                    prompt() {
                        return "star9:.$ ";
                    },
                    cwd() {
                        return ".";
                    },
                    lastStatus() {
                        return "0";
                    },
                };
            },
        },
    }, { simple: true });

    assert.equal(controller.prompt(), "star9:.$ ");
    assert.deepEqual(await controller.eval("version"), {
        status: 0,
        stdout: "0.1.0\n",
        stderr: "",
    });
    assert.equal(created, true);
});

test("Star9ShellController handles StarFS browser mount commands without bypassing core shell state", async () => {
    const calls = [];
    const mounts = [];
    const controller = createStar9Shell({
        mountStarFs(path, options) {
            mounts.push({ path, options });
        },
    }, {
        shell: {
            eval(line) {
                calls.push(line);
                return { status: 0, stdout: "", stderr: "" };
            },
            prompt() {
                return "star9:.$ ";
            },
            cwd() {
                return ".";
            },
        },
    });

    assert.deepEqual(await controller.eval("mount-starfs workspace agent-a"), {
        status: 0,
        stdout: "mounted starfs agent-a at workspace\n",
        stderr: "",
    });
    assert.deepEqual(calls, []);
    assert.deepEqual(mounts, [{
        path: "workspace",
        options: {
            id: "agent-a",
            storage: { backend: "opfs", root: "star9-shell-starfs-agent-a" },
        },
    }]);
});

test("Star9ShellController registers browser import services and mounts them through mount", async () => {
    const calls = [];
    const mounts = [];
    const controller = createStar9Shell({
        mountImport(path, url, options) {
            mounts.push({ path, url, options });
        },
    }, {
        shell: {
            eval(line) {
                calls.push(line);
                return { status: 0, stdout: "core\n", stderr: "" };
            },
            prompt() {
                return "star9:.$ ";
            },
            cwd() {
                return ".";
            },
        },
    });

    assert.deepEqual(await controller.eval("srv import!./export.html#star9 rem"), {
        status: 0,
        stdout: "srv rem import!./export.html#star9\n",
        stderr: "",
    });
    assert.deepEqual(await controller.eval("mount rem n/rem"), {
        status: 0,
        stdout: "mounted rem at n/rem\n",
        stderr: "",
    });
    assert.deepEqual(mounts, [{
        path: "n/rem",
        url: "./export.html#star9",
        options: { targetOrigin: "*" },
    }]);
    assert.deepEqual(calls, []);
});

test("Star9ShellController can srv -m a browser import service", async () => {
    const mounts = [];
    const controller = createStar9Shell({
        mountImport(path, url, options) {
            mounts.push({ path, url, options });
        },
    }, {
        shell: {
            eval() {
                throw new Error("core shell should not run browser import srv");
            },
        },
    });

    assert.deepEqual(await controller.eval("srv -m import!./export.html#star9 rem n/rem"), {
        status: 0,
        stdout: "srv rem import!./export.html#star9 mounted at n/rem\n",
        stderr: "",
    });
    assert.deepEqual(mounts, [{
        path: "n/rem",
        url: "./export.html#star9",
        options: { targetOrigin: "*" },
    }]);
});

test("Star9ShellController keeps raw browser TCP unavailable", async () => {
    const controller = createStar9Shell({}, {
        shell: {
            eval() {
                throw new Error("core shell should not run browser tcp srv");
            },
        },
    });

    assert.deepEqual(await controller.eval("srv tcp!host!564 rem"), {
        status: 1,
        stdout: "",
        stderr: "srv: tcp!host!564: raw TCP is not available in browsers\n",
    });
});

test("Star9ShellController registers and mounts browser network services", async () => {
    const mounts = [];
    const controller = createStar9Shell({
        mountBrowserService(path, source, options) {
            mounts.push({ path, source, options });
        },
    }, {
        shell: {
            eval() {
                throw new Error("core shell should not run browser ws srv");
            },
        },
    });

    assert.deepEqual(await controller.eval("srv ws!example.test!star9 rem"), {
        status: 0,
        stdout: "srv rem ws!example.test!star9\n",
        stderr: "",
    });
    assert.deepEqual(await controller.eval("mount rem n/rem"), {
        status: 0,
        stdout: "mounted rem at n/rem\n",
        stderr: "",
    });
    assert.deepEqual(mounts, [{
        path: "n/rem",
        source: "ws!example.test!star9",
        options: {
            family: "ws",
            source: "ws!example.test!star9",
            url: "ws://example.test/star9",
        },
    }]);
});

test("Star9ShellController can srv -m browser WebTransport services when provider is configured", async () => {
    const mounts = [];
    const controller = createStar9Shell({
        mountNetworkService(path, source, options) {
            mounts.push({ path, source, options });
        },
    }, {
        shell: {
            eval() {
                throw new Error("core shell should not run browser webtransport srv");
            },
        },
    });

    assert.deepEqual(await controller.eval("srv -m webtransport!example.test!star9 rem n/rem"), {
        status: 0,
        stdout: "srv rem webtransport!example.test!star9 mounted at n/rem\n",
        stderr: "",
    });
    assert.deepEqual(mounts, [{
        path: "n/rem",
        source: "webtransport!example.test!star9",
        options: {
            family: "webtransport",
            source: "webtransport!example.test!star9",
            url: "https://example.test/star9",
        },
    }]);
});

test("Star9ShellController reports unavailable OPFS as a command error", async () => {
    const previousNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
    Object.defineProperty(globalThis, "navigator", {
        configurable: true,
        value: {},
    });
    try {
        const controller = createStar9Shell({}, {
            shell: {
                eval() {
                    throw new Error("core shell should not run mount-opfs");
                },
            },
        });
        assert.deepEqual(await controller.eval("mount-opfs workspace"), {
            status: 1,
            stdout: "",
            stderr: "mount-opfs: OPFS is not available in this browser\n",
        });
    } finally {
        if (previousNavigator) {
            Object.defineProperty(globalThis, "navigator", previousNavigator);
        } else {
            delete globalThis.navigator;
        }
    }
});

test("Star9ShellController runs browser rc worker pipelines through graph provider", async () => {
    const files = new Map();
    const started = [];
    const controller = createStar9Shell({
        mkdir(path) {
            files.set(path, "<dir>");
        },
        writeText(path, value) {
            files.set(path, String(value));
        },
        startBrowserWorker(source, options) {
            started.push({ source, options });
            const stdin = options.bootstrapMessage?.stdin_text || "";
            const stdout = options.module.endsWith("producer.mjs")
                ? "browser-pipe-ok\n"
                : stdin;
            return {
                taskId: `task-${started.length}`,
                workerId: options.workerId,
                messages: [stdout],
                done: Promise.resolve({ exitCode: 0, stdout, stderr: "" }),
                close() {},
                cancel() {},
            };
        },
    }, {
        shell: {
            eval() {
                throw new Error("core rc shell should not run browser worker graph");
            },
            cwd() {
                return ".";
            },
            prompt() {
                return "star9:.$ ";
            },
        },
    });

    assert.deepEqual(await controller.eval("worker producer.mjs | worker cat.mjs"), {
        status: 0,
        stdout: "browser-pipe-ok\n",
        stderr: "",
    });
    assert.equal(started.length, 2);
    assert.equal(started[1].options.bootstrapMessage.stdin_text, "browser-pipe-ok\n");
    assert.equal(files.get(".rc/graphs/browser-rcgraph1/status"), "0|0\n");
    assert.equal(files.get(".rc/graphs/browser-rcgraph1/state"), "exited\n");
});

test("Star9ShellController runs browser rc background worker jobs and wait", async () => {
    const controller = createStar9Shell({
        mkdir() {},
        writeText() {},
        startBrowserWorker(_source, options) {
            return {
                taskId: options.workerId,
                workerId: options.workerId,
                messages: ["bg-ok\n"],
                done: Promise.resolve({ exitCode: 0, stdout: "bg-ok\n", stderr: "" }),
                close() {},
                cancel() {},
            };
        },
    }, {
        shell: {
            eval() {
                throw new Error("core rc shell should not run browser background graph");
            },
            cwd() {
                return ".";
            },
            prompt() {
                return "star9:.$ ";
            },
        },
    });

    assert.deepEqual(await controller.eval("worker producer.mjs &"), {
        status: 0,
        stdout: "[1]\n",
        stderr: "",
    });
    assert.deepEqual(await controller.eval("wait 1"), {
        status: 0,
        stdout: "bg-ok\n[1] 0\n",
        stderr: "",
    });
});
