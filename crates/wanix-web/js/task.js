import { WanixElement } from "./base.js";

export class TaskElement extends WanixElement {
    constructor() {
        super();
        this.taskId = null;
    }

    connectedCallback() {
        this.alias = this.getAttribute("alias") || this.getAttribute("id") || "";
        this.type = (this.getAttribute("type") || "auto").trim();
        this.cmd = this.getAttribute("cmd") || "";
        this.env = this.getAttribute("env") || "";
        this.wd = this.getAttribute("wd") || "";
        this.autostart = this.hasAttribute("start");
        super.connectedCallback();
    }

    async _awake() {
        await this.allocate();
        if (this.autostart) {
            await this.start();
        }
        this.dispatchEvent(
            new CustomEvent("ready", {
                bubbles: true,
                detail: { taskId: this.taskId },
            }),
        );
    }

    async allocate() {
        if (this.taskId) {
            return this.taskId;
        }

        const taskId = this._system.readText(`${this._taskpath}/new/${this.type}`).trim();
        const taskPath = `${this._taskpath}/${taskId}`;

        if (this.cmd) {
            this._system.writeText(`${taskPath}/cmd`, this.cmd);
        }
        if (this.env) {
            this._system.writeText(`${taskPath}/env`, spaceToNewline(this.env));
        }
        if (this.wd) {
            this._system.writeText(`${taskPath}/dir`, this.wd);
        }
        if (this.alias) {
            this._system.writeText(`${taskPath}/alias`, this.alias);
        }

        this.taskId = taskId;
        await this._system.applyBindings(this.querySelectorAll(":scope > wanix-bind"), taskId, this._taskpath);
        return taskId;
    }

    async start() {
        const taskId = await this.allocate();
        this._system.writeText(`${this._taskpath}/${taskId}/ctl`, "start");
        return taskId;
    }
}

function spaceToNewline(input) {
    const tokens = [];
    let current = "";
    let inQuotes = false;

    for (const char of input) {
        if (char === "'") {
            inQuotes = !inQuotes;
            continue;
        }
        if (char === " " && !inQuotes) {
            if (current) {
                tokens.push(current);
                current = "";
            }
            continue;
        }
        current += char;
    }

    if (current) {
        tokens.push(current);
    }

    return tokens.join("\n");
}
