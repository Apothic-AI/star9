import { WanixElement } from "./base.js";

export class TerminalElement extends WanixElement {
    constructor() {
        super();
        this.termId = null;
        this.isReady = false;
        this.ready = new Promise((resolve, reject) => {
            this._resolveReady = resolve;
            this._rejectReady = reject;
        });
    }

    async _awake() {
        try {
            this.termId = String(this.getAttribute("term-id") || "").trim();
            this.raw = this.hasAttribute("raw");
            if (!this.termId) {
                this.termId = (await this._system.readText("#term/new")).trim();
                this.setAttribute("term-id", this.termId);
            }
            await this.refresh();
            this.isReady = true;
            this._resolveReady(this);
            this.dispatchEvent(new CustomEvent("ready", {
                bubbles: true,
                detail: { terminal: this, termId: this.termId },
            }));
        } catch (error) {
            this._rejectReady(error);
            this.dispatchEvent(new CustomEvent("error", {
                bubbles: true,
                detail: { error },
            }));
        }
    }

    path(name) {
        if (!this.termId) {
            throw new Error("wanix-terminal is not ready");
        }
        return `#term/${this.termId}/${name}`;
    }

    async writeData(text) {
        await this._system.writeExistingText(this.path("data"), String(text));
        return this.refresh();
    }

    async writeProgram(text) {
        await this._system.writeExistingText(this.path(this.raw ? "raw" : "program"), String(text));
        return this.refresh();
    }

    async resize(cols, rows) {
        const size = `${Number(cols) || 0}x${Number(rows) || 0}`;
        await this._system.writeExistingText(this.path("size"), size);
        await this._system.writeExistingText(this.path("winch/data"), size);
        return this.refresh();
    }

    async clear() {
        await this._system.writeExistingText(this.path("ctl"), "clear");
        return this.refresh();
    }

    async reset() {
        await this._system.writeExistingText(this.path("ctl"), "reset");
        return this.refresh();
    }

    async refresh() {
        const [screen, size, state] = await Promise.all([
            this._system.readText(this.path("screen")),
            this._system.readText(this.path("size")),
            this._system.readText(this.path("state")),
        ]);
        this.dataset.termId = this.termId;
        this.dataset.size = size.trim();
        this.dataset.state = state.trim();
        this.textContent = screen;
        this.dispatchEvent(new CustomEvent("change", {
            bubbles: true,
            detail: {
                terminal: this,
                termId: this.termId,
                screen,
                size: size.trim(),
                state: state.trim(),
            },
        }));
        return { screen, size: size.trim(), state: state.trim() };
    }
}
