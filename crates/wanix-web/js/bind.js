const textEncoder = new TextEncoder();

export class BindElement extends HTMLElement {
    constructor() {
        super();
        this._plan = null;
    }

    connectedCallback() {
        this.style.display = "none";
    }

    toBinding() {
        const dst = requireAttribute(this, "dst");
        const kind = normalizeKind(this.getAttribute("kind") || this.getAttribute("type") || "ns");
        const src = bindingSourceFor(kind, this.getAttribute("src"));
        const storage = parseStorageDescriptor(this);

        if ((kind === "archive" || kind === "import") && !src && !storage) {
            throw new Error(`wanix-bind kind=${JSON.stringify(kind)} requires a src attribute`);
        }

        return {
            dst,
            src,
            kind,
            storage,
        };
    }

    plan() {
        if (!this._plan) {
            this._plan = this._buildPlan();
        }
        return this._plan;
    }

    async _buildPlan() {
        const binding = this.toBinding();

        if (binding.storage) {
            return { binding };
        }

        switch (binding.kind) {
        case "file":
            return {
                binding,
                fileBytes: await loadFileBytes(this, binding),
            };
        case "archive":
            return {
                binding,
                archiveBytes: await loadFetchBytes(binding.src, "archive"),
            };
        case "import":
            return {
                binding,
                importPlaceholder: createImportPlaceholder(binding.src),
            };
        case "ns":
        default:
            return { binding };
        }
    }
}

function normalizeKind(kind) {
    const normalized = String(kind).trim().toLowerCase();
    if (normalized === "fetch") {
        return "file";
    }
    if (["ns", "file", "archive", "import"].includes(normalized)) {
        return normalized;
    }
    throw new Error(`Unsupported wanix-bind kind ${JSON.stringify(kind)}`);
}

function bindingSourceFor(kind, rawValue) {
    const value = rawValue?.trim();
    if (!value) {
        return null;
    }
    if (kind === "ns") {
        return value;
    }
    return new URL(value, document.baseURI).href;
}

function parseStorageDescriptor(element) {
    const inlineJson =
        element.getAttribute("storage") ||
        element.querySelector(':scope > script[type="application/json"][data-storage]')?.textContent;

    if (!inlineJson) {
        return null;
    }

    try {
        return JSON.parse(inlineJson);
    } catch (error) {
        throw new Error(`Invalid wanix-bind storage JSON: ${error instanceof Error ? error.message : String(error)}`);
    }
}

async function loadFileBytes(element, binding) {
    if (binding.src) {
        return loadFetchBytes(binding.src, "file");
    }
    return textEncoder.encode(element.textContent || "");
}

async function loadFetchBytes(url, label) {
    const response = await fetch(url);
    if (!response.ok) {
        throw new Error(`Failed to fetch ${label} binding ${JSON.stringify(url)}: HTTP ${response.status}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

function createImportPlaceholder(src) {
    return {
        src,
        async requestPort() {
            return requestImportPort(src);
        },
    };
}

async function requestImportPort(src) {
    return new Promise((resolve, reject) => {
        const iframe = document.createElement("iframe");
        iframe.style.display = "none";
        iframe.src = src;
        iframe.onload = () => {
            const channel = new MessageChannel();
            channel.port1.onmessage = (event) => {
                iframe.remove();
                resolve(event.data);
            };
            try {
                iframe.contentWindow.postMessage(
                    {
                        request: "wanix-import",
                        responder: channel.port2,
                    },
                    "*",
                    [channel.port2],
                );
            } catch (error) {
                iframe.remove();
                reject(error);
            }
        };
        iframe.onerror = () => {
            iframe.remove();
            reject(new Error(`Failed to load import binding source ${JSON.stringify(src)}`));
        };
        document.body.append(iframe);
    });
}

function requireAttribute(element, name) {
    const value = element.getAttribute(name)?.trim();
    if (!value) {
        throw new Error(`wanix-bind is missing required ${name} attribute`);
    }
    return value;
}
