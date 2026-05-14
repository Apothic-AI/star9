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

test("Star9ShellController delegates normal commands to the wasm shell facade", async () => {
    const calls = [];
    const controller = createStar9Shell({
        system: {
            createShell() {
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

test("Star9ShellController can request an rc shell facade", async () => {
    let created = false;
    const controller = createStar9Shell({
        system: {
            createRcShell() {
                created = true;
                return {
                    eval(line) {
                        assert.equal(line, "x=(a b); echo $x");
                        return { status: "0", success: true, stdout: "a b\n", stderr: "" };
                    },
                    prompt() {
                        return "rc:.$ ";
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
    }, { rc: true });

    assert.equal(controller.prompt(), "rc:.$ ");
    assert.deepEqual(await controller.eval("x=(a b); echo $x"), {
        status: 0,
        stdout: "a b\n",
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
