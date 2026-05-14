import { Star9Element } from "./base.js";

const textDecoder = new TextDecoder();

export function createStar9Shell(system, options = {}) {
    return new Star9ShellController(system, options);
}

export class Star9ShellController {
    constructor(system, options = {}) {
        if (!system) {
            throw new Error("createStar9Shell requires a star9-system instance");
        }
        this.system = system;
        this.shell = options.shell || system.system?.createShell?.() || system.createShell?.();
        if (!this.shell || typeof this.shell.eval !== "function") {
            throw new Error("star9 shell facade is not available");
        }
    }

    prompt() {
        if (typeof this.shell.prompt === "function") {
            return this.shell.prompt();
        }
        return "star9:.$ ";
    }

    cwd() {
        if (typeof this.shell.cwd === "function") {
            return this.shell.cwd();
        }
        return ".";
    }

    lastStatus() {
        if (typeof this.shell.lastStatus === "function") {
            return Number(this.shell.lastStatus());
        }
        return 0;
    }

    async eval(line) {
        const browserResult = await this.#tryBrowserCommand(line);
        if (browserResult) {
            return browserResult;
        }
        const result = await Promise.resolve(this.shell.eval(line));
        return normalizeShellResult(result);
    }

    async #tryBrowserCommand(line) {
        const words = splitShellWords(line);
        if (words.length === 0) {
            return null;
        }
        switch (words[0]) {
        case "mount-opfs":
            return this.#mountOpfs(words);
        case "mount-starfs":
            return this.#mountStarFs(words);
        case "import":
            return this.#mountImport(words);
        case "browser-worker":
        case "worker-browser":
            return this.#startBrowserWorker(words);
        case "download":
            return this.#download(words);
        default:
            return null;
        }
    }

    async #mountOpfs(words) {
        if (words.length < 2 || words.length > 3) {
            return failure("usage: mount-opfs <path> [root]\n", 2);
        }
        if (!globalThis.navigator?.storage?.getDirectory) {
            return failure("mount-opfs: OPFS is not available in this browser\n");
        }
        const dst = words[1];
        const root = words[2] || `star9-shell-${dst.replace(/[^a-zA-Z0-9_.-]+/g, "-")}`;
        await this.system.mountStorageExport(dst, { backend: "opfs", root });
        return success(`mounted opfs ${root} at ${dst}\n`);
    }

    async #mountStarFs(words) {
        if (words.length < 2 || words.length > 3) {
            return failure("usage: mount-starfs <path> [id]\n", 2);
        }
        const dst = words[1];
        const id = words[2] || "shell";
        await this.system.mountStarFs(dst, { id, storage: { backend: "opfs", root: `star9-shell-starfs-${id}` } });
        return success(`mounted starfs ${id} at ${dst}\n`);
    }

    async #mountImport(words) {
        if (words.length !== 3) {
            return failure("usage: import <path> <url#system>\n", 2);
        }
        await this.system.mountImport(words[1], words[2], { targetOrigin: "*" });
        return success(`mounted import ${words[2]} at ${words[1]}\n`);
    }

    async #startBrowserWorker(words) {
        if (words.length < 2) {
            return failure("usage: browser-worker <worker-url> [module] [args...]\n", 2);
        }
        const workerUrl = words[1];
        const module = words[2] || "../../../tests/fixtures/js-wasm-execution-runner.mjs";
        const args = words.slice(3);
        const task = await this.system.startBrowserWorker(workerUrl, {
            workerId: `shell-worker-${Date.now()}`,
            module,
            args,
            cwd: ".",
        });
        return success(`browser-worker task=${task.taskId} worker=${task.workerId}\n`);
    }

    async #download(words) {
        if (words.length !== 2) {
            return failure("usage: download <path>\n", 2);
        }
        const bytes = await this.system.readFile(words[1]);
        const blob = new Blob([bytes], { type: "application/octet-stream" });
        const url = URL.createObjectURL(blob);
        const anchor = document.createElement("a");
        anchor.href = url;
        anchor.download = words[1].split("/").pop() || "star9-download";
        anchor.click();
        URL.revokeObjectURL(url);
        return success(`downloaded ${words[1]}\n`);
    }
}

export class ShellElement extends Star9Element {
    constructor() {
        super();
        this.controller = null;
        this.history = [];
        this.historyIndex = 0;
        this.ready = new Promise((resolve, reject) => {
            this._resolveReady = resolve;
            this._rejectReady = reject;
        });
    }

    async _awake() {
        try {
            this.controller = createStar9Shell(this._system);
            this.#render();
            this.#append("Star 9 shell ready\n", "system");
            this._resolveReady(this);
            this.dispatchEvent(new CustomEvent("ready", {
                bubbles: true,
                detail: { shell: this },
            }));
        } catch (error) {
            this._rejectReady(error);
            this.dispatchEvent(new CustomEvent("error", {
                bubbles: true,
                detail: { error },
            }));
        }
    }

    async eval(line) {
        const command = String(line || "");
        this.#append(`${this.controller.prompt()}${command}\n`, "command");
        const result = await this.controller.eval(command);
        if (result.stdout) {
            this.#append(result.stdout, "stdout");
        }
        if (result.stderr) {
            this.#append(result.stderr, "stderr");
        }
        this.dataset.cwd = this.controller.cwd();
        this.dataset.status = String(result.status);
        return result;
    }

    #render() {
        this.classList.add("star9-shell");
        this.innerHTML = `
            <style>
                star9-shell {
                    display: grid;
                    min-height: 100vh;
                    grid-template-rows: minmax(0, 1fr) auto;
                    background: #101113;
                    color: #f4f4f0;
                    font: 14px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
                }
                star9-shell .transcript {
                    margin: 0;
                    padding: 16px;
                    overflow: auto;
                    white-space: pre-wrap;
                    word-break: break-word;
                }
                star9-shell form {
                    display: grid;
                    grid-template-columns: auto minmax(0, 1fr);
                    gap: 8px;
                    align-items: center;
                    border-top: 1px solid #34363a;
                    padding: 10px 12px;
                    background: #181a1d;
                }
                star9-shell .prompt {
                    color: #8fc7ff;
                }
                star9-shell input {
                    min-width: 0;
                    border: 0;
                    outline: none;
                    background: transparent;
                    color: inherit;
                    font: inherit;
                }
                star9-shell .stderr {
                    color: #ff9d9d;
                }
                star9-shell .system {
                    color: #a7e3b1;
                }
            </style>
            <pre class="transcript" part="transcript"></pre>
            <form part="form" autocomplete="off">
                <span class="prompt" part="prompt"></span>
                <input part="input" spellcheck="false" autocomplete="off" />
            </form>
        `;
        this._transcript = this.querySelector(".transcript");
        this._prompt = this.querySelector(".prompt");
        this._input = this.querySelector("input");
        this._form = this.querySelector("form");
        this.#refreshPrompt();
        this._form.addEventListener("submit", (event) => {
            event.preventDefault();
            void this.#submit();
        });
        this._input.addEventListener("keydown", (event) => this.#handleKey(event));
        this._input.focus();
    }

    async #submit() {
        const line = this._input.value;
        this._input.value = "";
        if (line.trim()) {
            this.history.push(line);
            this.historyIndex = this.history.length;
        }
        await this.eval(line);
        this.#refreshPrompt();
    }

    #handleKey(event) {
        if (event.key === "ArrowUp") {
            event.preventDefault();
            this.historyIndex = Math.max(0, this.historyIndex - 1);
            this._input.value = this.history[this.historyIndex] || "";
            this._input.setSelectionRange(this._input.value.length, this._input.value.length);
        } else if (event.key === "ArrowDown") {
            event.preventDefault();
            this.historyIndex = Math.min(this.history.length, this.historyIndex + 1);
            this._input.value = this.history[this.historyIndex] || "";
            this._input.setSelectionRange(this._input.value.length, this._input.value.length);
        } else if (event.key === "l" && (event.ctrlKey || event.metaKey)) {
            event.preventDefault();
            this._transcript.textContent = "";
        }
    }

    #refreshPrompt() {
        this._prompt.textContent = this.controller.prompt();
        this.dataset.cwd = this.controller.cwd();
    }

    #append(text, kind) {
        const span = document.createElement("span");
        span.className = kind;
        span.textContent = text;
        this._transcript.append(span);
        this._transcript.scrollTop = this._transcript.scrollHeight;
    }
}

function normalizeShellResult(result) {
    return {
        status: Number(result?.status ?? 0),
        stdout: stringifyOutput(result?.stdout ?? ""),
        stderr: stringifyOutput(result?.stderr ?? ""),
    };
}

function success(stdout = "") {
    return { status: 0, stdout, stderr: "" };
}

function failure(stderr, status = 1) {
    return { status, stdout: "", stderr };
}

function stringifyOutput(value) {
    if (value instanceof Uint8Array) {
        return textDecoder.decode(value);
    }
    return String(value ?? "");
}

export function splitShellWords(line) {
    const words = [];
    let current = "";
    let quote = null;
    let escaped = false;
    for (let i = 0; i < String(line || "").length; i += 1) {
        const ch = line[i];
        if (escaped) {
            current += ch;
            escaped = false;
            continue;
        }
        if (quote === "'") {
            if (ch === "'") quote = null;
            else current += ch;
            continue;
        }
        if (quote === "\"") {
            if (ch === "\"") quote = null;
            else if (ch === "\\") escaped = true;
            else current += ch;
            continue;
        }
        if (ch === "\\" ) {
            escaped = true;
        } else if (ch === "'" || ch === "\"") {
            quote = ch;
        } else if (/\s/.test(ch)) {
            if (current) {
                words.push(current);
                current = "";
            }
        } else {
            current += ch;
        }
    }
    if (escaped) current += "\\";
    if (current) words.push(current);
    return words;
}

