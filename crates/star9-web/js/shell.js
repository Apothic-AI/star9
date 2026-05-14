import { Star9Element } from "./base.js";

const textDecoder = new TextDecoder();
const DEFAULT_BROWSER_EXECUTION_WORKER_URL = new URL("./star9-execution-worker.js?rc-graph=2", import.meta.url).href;

export function createStar9Shell(system, options = {}) {
    return new Star9ShellController(system, options);
}

export class Star9ShellController {
    constructor(system, options = {}) {
        if (!system) {
            throw new Error("createStar9Shell requires a star9-system instance");
        }
        this.system = system;
        this.rc = options.rc === undefined ? !Boolean(options.simple) : Boolean(options.rc);
        this.services = new Map();
        this.browserGraphs = new BrowserRcGraphProvider(this, options.browserGraphs || options);
        this.shell = options.shell || this.#createFacadeShell(system);
        if (!this.shell || typeof this.shell.eval !== "function") {
            throw new Error("star9 shell facade is not available");
        }
    }

    #createFacadeShell(system) {
        if (this.rc) {
            return system.system?.createRcShell?.() || system.createRcShell?.();
        }
        return system.system?.createShell?.() || system.createShell?.();
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
        const graphResult = await this.browserGraphs.tryEval(line);
        if (graphResult) {
            return graphResult;
        }
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
        case "srv":
            return this.#browserSrv(words);
        case "mount":
            return this.#browserMount(words);
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

    async #browserSrv(words) {
        const parsed = parseBrowserSrv(words);
        if (!parsed) {
            return null;
        }
        if (parsed.error) {
            return failure(parsed.error, 2);
        }
        if (parsed.kind === "raw-tcp") {
            return failure(`srv: ${parsed.source}: raw TCP is not available in browsers\n`);
        }
        if (parsed.kind === "provider-missing") {
            return failure(`srv: ${parsed.source}: browser provider not configured\n`);
        }
        this.services.set(parsed.name, parsed);
        let mounted = "";
        if (parsed.mountpoint) {
            try {
                await this.#mountBrowserService(parsed, parsed.mountpoint);
            } catch (error) {
                return failure(`${errorMessage(error)}\n`);
            }
            mounted = ` mounted at ${parsed.mountpoint}`;
        }
        return success(`srv ${parsed.name} ${parsed.source}${mounted}\n`);
    }

    async #browserMount(words) {
        const parsed = parseMountWords(words);
        if (!parsed) {
            return null;
        }
        if (parsed.error) {
            return failure(parsed.error, 2);
        }
        const service = this.services.get(parsed.service);
        if (!service) {
            return null;
        }
        try {
            await this.#mountBrowserService(service, parsed.mountpoint);
        } catch (error) {
            return failure(`${errorMessage(error)}\n`);
        }
        return success(`mounted ${parsed.service} at ${parsed.mountpoint}\n`);
    }

    async #mountBrowserService(service, mountpoint) {
        if (service.kind === "import") {
            await this.system.mountImport(mountpoint, service.url, { targetOrigin: "*" });
            return;
        }
        if (service.kind === "browser-network") {
            const mountService = this.system.mountBrowserService || this.system.mountNetworkService;
            if (typeof mountService !== "function") {
                throw new Error(`browser network service provider is not configured: ${service.source}`);
            }
            await mountService.call(this.system, mountpoint, service.source, {
                family: service.family,
                source: service.source,
                url: service.url,
            });
            return;
        }
        throw new Error(`unsupported browser service kind: ${service.kind}`);
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

class BrowserRcGraphProvider {
    constructor(controller, options = {}) {
        this.controller = controller;
        this.system = controller.system;
        this.workerSource = String(options.workerSource || DEFAULT_BROWSER_EXECUTION_WORKER_URL);
        this.nextGraphId = 1;
        this.nextJobId = 1;
        this.jobs = new Map();
    }

    async tryEval(line) {
        const text = String(line || "").trim();
        if (!this.controller.rc || !text) {
            return null;
        }
        const wait = parseBrowserWait(text);
        if (wait) {
            return this.#wait(wait.jobId);
        }
        const parsed = parseBrowserWorkerGraph(text);
        if (!parsed) {
            return null;
        }
        if (parsed.background) {
            const id = this.nextJobId++;
            const graph = this.#startGraph(parsed.stages, { background: true, jobId: id });
            this.jobs.set(id, graph);
            return success(`[${id}]\n`);
        }
        const result = await this.#startGraph(parsed.stages, { background: false }).promise;
        return graphResultToShell(result);
    }

    #startGraph(stages, options = {}) {
        const graphId = `browser-rcgraph${this.nextGraphId++}`;
        const root = `.rc/graphs/${graphId}`;
        const controllers = [];
        const record = {
            id: options.jobId ?? null,
            graphId,
            root,
            controllers,
            promise: null,
            cancel: (note = "term") => {
                for (const controller of controllers) {
                    controller.cancel?.(note);
                }
                void this.#writeGraph(root, {
                    state: "signaled\n",
                    status: `signal:${note}\n`,
                    notes: `${note}\n`,
                });
            },
            cleanup: async () => {
                await this.#writeGraph(root, { state: "cleaned\n" });
                if (typeof this.system.unmount === "function") {
                    try {
                        this.system.unmount(root);
                    } catch {
                        // Best effort: browser graph files are diagnostic, not the authority.
                    }
                }
            },
        };
        record.promise = this.#executeGraph(root, stages, controllers, options);
        return record;
    }

    async #executeGraph(root, stages, controllers, options) {
        await this.#ensureGraphFiles(root, options);
        let input = "";
        let stdout = "";
        let stderr = "";
        const statuses = [];
        const taskIds = [];
        await this.#writeGraph(root, { state: "running\n" });
        for (let index = 0; index < stages.length; index += 1) {
            const stage = stages[index];
            const worker = await this.system.startBrowserWorker(this.workerSource, {
                workerId: `${root.replace(/[^a-zA-Z0-9_.-]+/g, "-")}-stage${index}-${Date.now()}-${Math.floor(Math.random() * 1_000_000)}`,
                module: stage.module,
                args: stage.args,
                cwd: this.controller.cwd(),
                bootstrapMessage: {
                    stdin_text: input,
                },
            });
            controllers.push(worker);
            taskIds.push(worker.taskId);
            const result = await worker.done;
            statuses.push(result.exitCode === 0 ? "0" : String(result.exitCode || 1));
            stderr += result.stderr || "";
            input = result.stdout || "";
            if (index === stages.length - 1) {
                stdout += input;
            }
            worker.close();
        }
        const statusText = statuses.join("|") || "0";
        await this.#writeGraph(root, {
            tasks: taskIds.join("\n") + (taskIds.length ? "\n" : ""),
            status: `${statusText}\n`,
            state: statuses.every((status) => status === "0") ? "exited\n" : "failed\n",
        });
        return {
            id: options.jobId ?? null,
            statusText,
            stdout,
            stderr,
        };
    }

    async #wait(jobId) {
        const requested = jobId == null ? Array.from(this.jobs.keys()) : [jobId];
        if (requested.length === 0) {
            return null;
        }
        let stdout = "";
        let stderr = "";
        let status = 0;
        for (const id of requested) {
            const job = this.jobs.get(id);
            if (!job) {
                return failure(`wait: ${id}: no such browser job\n`);
            }
            const result = await job.promise;
            stdout += result.stdout || "";
            stderr += result.stderr || "";
            stdout += `[${id}] ${result.statusText || "0"}\n`;
            if (!isRcStatusSuccess(result.statusText)) {
                status = 1;
            }
            this.jobs.delete(id);
        }
        return { status, stdout, stderr };
    }

    async #ensureGraphFiles(root, options) {
        for (const path of [".rc", ".rc/graphs", root]) {
            try {
                await this.system.mkdir?.(path);
            } catch {
                // Existing or unavailable graph directories should not block execution.
            }
        }
        await this.#writeGraph(root, {
            kind: `${options.background ? "Background" : "Pipeline"}\n`,
            job: `${options.jobId ?? "-"}\n`,
            tasks: "",
            notes: "",
            state: "planned\n",
            status: "planned\n",
            ctl: "",
        });
    }

    async #writeGraph(root, files) {
        for (const [name, value] of Object.entries(files)) {
            try {
                await this.system.writeText?.(`${root}/${name}`, String(value));
            } catch {
                // Diagnostic graph files are best-effort for browser facade hosts.
            }
        }
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
            const options = this.hasAttribute("simple")
                ? { simple: true }
                : { rc: true };
            this.controller = createStar9Shell(this._system, options);
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
        status: result?.success === true ? 0 : Number(result?.status ?? 0),
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

function graphResultToShell(result) {
    return {
        status: isRcStatusSuccess(result.statusText) ? 0 : 1,
        stdout: result.stdout || "",
        stderr: result.stderr || "",
    };
}

function isRcStatusSuccess(statusText) {
    return String(statusText || "0")
        .split("|")
        .every((part) => part === "" || part === "0");
}

function stringifyOutput(value) {
    if (value instanceof Uint8Array) {
        return textDecoder.decode(value);
    }
    return String(value ?? "");
}

function errorMessage(error) {
    return error instanceof Error ? error.message : String(error);
}

function parseBrowserWait(text) {
    const words = splitShellWords(text);
    if (words[0] !== "wait" || words.length > 2) {
        return null;
    }
    if (words.length === 1) {
        return { jobId: null };
    }
    const jobId = Number(words[1]);
    if (!Number.isInteger(jobId) || jobId <= 0) {
        return null;
    }
    return { jobId };
}

function parseBrowserWorkerGraph(text) {
    const trimmed = String(text || "").trim();
    const background = trimmed.endsWith("&");
    const source = background ? trimmed.slice(0, -1).trim() : trimmed;
    if (!source || /[;<>]/.test(source)) {
        return null;
    }
    const segments = splitUnquoted(source, "|");
    if (segments.length === 0) {
        return null;
    }
    const stages = [];
    for (const segment of segments) {
        const words = splitShellWords(segment.trim());
        if (words.length === 0) {
            return null;
        }
        if (words[0] === "worker" && words.length >= 2) {
            stages.push({ module: words[1], args: words.slice(2) });
        } else if (isBrowserWorkerModule(words[0])) {
            stages.push({ module: words[0], args: words.slice(1) });
        } else {
            return null;
        }
    }
    return { background, stages };
}

function isBrowserWorkerModule(value) {
    return /\.(?:m?js|wasm)(?:[?#].*)?$/i.test(String(value || ""));
}

function splitUnquoted(text, separator) {
    const out = [];
    let current = "";
    let quote = null;
    let escaped = false;
    for (const ch of String(text || "")) {
        if (escaped) {
            current += ch;
            escaped = false;
            continue;
        }
        if (quote === "'") {
            if (ch === "'") quote = null;
            current += ch;
            continue;
        }
        if (quote === "\"") {
            if (ch === "\"") quote = null;
            current += ch;
            escaped = ch === "\\";
            continue;
        }
        if (ch === "'" || ch === "\"") {
            quote = ch;
            current += ch;
            continue;
        }
        if (ch === "\\") {
            current += ch;
            escaped = true;
            continue;
        }
        if (ch === separator) {
            out.push(current);
            current = "";
            continue;
        }
        current += ch;
    }
    out.push(current);
    return out;
}

function parseBrowserSrv(words) {
    const args = words.slice(1);
    let mount = false;
    while (args[0]?.startsWith("-")) {
        const flag = args.shift();
        if (flag === "-m") {
            mount = true;
        } else {
            return { error: `usage: srv [-m] <service-address> <name> [mountpoint]\n` };
        }
    }
    if (args.length === 0) {
        return null;
    }
    const source = args[0];
    if (!isBrowserServiceSource(source)) {
        return null;
    }
    if (args.length < 2 || args.length > 3 || (mount && args.length !== 3) || (!mount && args.length === 3)) {
        return { error: "usage: srv [-m] <service-address> <name> [mountpoint]\n" };
    }
    const name = args[1];
    const mountpoint = mount ? args[2] : null;
    if (source.startsWith("import!")) {
        const url = source.slice("import!".length);
        if (!url) {
            return { error: "srv: import service requires a url#system address\n" };
        }
        return { kind: "import", source, name, url, mountpoint };
    }
    if (source.startsWith("tcp!")) {
        return { kind: "raw-tcp", source, name, mountpoint };
    }
    if (source.startsWith("ws!") || source.startsWith("wss!") || source.startsWith("webtransport!")) {
        const parsed = parseBrowserNetworkServiceSource(source);
        if (parsed.error) {
            return { error: `srv: ${source}: ${parsed.error}\n` };
        }
        return { kind: "browser-network", source, name, mountpoint, ...parsed };
    }
    return { kind: "provider-missing", source, name, mountpoint };
}

function parseMountWords(words) {
    const args = words.slice(1);
    while (args[0]?.startsWith("-")) {
        const flag = args.shift();
        if (!["-a", "-b", "-c", "-n", "-C"].includes(flag)) {
            return { error: "usage: mount [-a|-b|-c|-n|-C] <service> <mountpoint> [aname]\n" };
        }
    }
    if (args.length === 0) {
        return null;
    }
    if (args.length < 2) {
        return { error: "usage: mount [-a|-b|-c|-n|-C] <service> <mountpoint> [aname]\n" };
    }
    return { service: args[0], mountpoint: args[1] };
}

function isBrowserServiceSource(source) {
    return source.startsWith("import!")
        || source.startsWith("tcp!")
        || source.startsWith("ws!")
        || source.startsWith("wss!")
        || source.startsWith("webtransport!");
}

function parseBrowserNetworkServiceSource(source) {
    const bang = source.indexOf("!");
    const family = source.slice(0, bang);
    const rest = source.slice(bang + 1);
    if (!rest) {
        return { error: "missing service address" };
    }
    if (family === "ws" || family === "wss") {
        const url = familyUrl(family, rest);
        return typeof url === "string" ? { family, url } : url;
    }
    if (family === "webtransport") {
        const url = familyUrl("https", rest);
        return typeof url === "string" ? { family, url } : url;
    }
    return { error: "unknown browser network service family" };
}

function familyUrl(scheme, address) {
    if (/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(address)) {
        return address;
    }
    const [host, ...pathParts] = address.split("!");
    const path = pathParts.join("/");
    if (!host) {
        return { error: "missing service host" };
    }
    if (!path) {
        return `${scheme}://${host}`;
    }
    return `${scheme}://${host}${path.startsWith("/") ? "" : "/"}${path}`;
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
