// Repotory Agent Shell — first screen.
//
// The window asks the Rust side for one status payload and renders it. The
// frontend owns no runtime knowledge: which model backends exist, and whether
// the Repotory MCP server is installed, are both facts the backend owns.
import { invoke } from "@tauri-apps/api/core";

type BackendStatus = {
  id: string;
  available: boolean;
  path: string | null;
  state: string;
};

type MemoryStatus = {
  available: boolean;
  path: string | null;
  tools: string[];
};

type ShellStatus = {
  backends: BackendStatus[];
  defaultBackend: string;
  defaultModel: string;
  memory: MemoryStatus;
};

const BACKEND_LABELS: Record<string, string> = {
  ollama: "Ollama",
  "llama.cpp": "llama.cpp",
  mlx: "MLX",
};

function pick<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`missing element: ${selector}`);
  return found;
}

function setChip(el: HTMLElement, text: string, variant: "ok" | "warn" | "quiet") {
  el.textContent = text;
  el.classList.remove("chip--ok", "chip--warn", "chip--quiet");
  el.classList.add(`chip--${variant}`);
}

function renderMemory(status: ShellStatus) {
  const chip = pick<HTMLElement>("#memory-chip");
  const path = pick<HTMLElement>("#memory-path");
  const tools = pick<HTMLElement>("#memory-tools");

  if (status.memory.available) {
    setChip(chip, "已安装", "ok");
    path.textContent = status.memory.path ?? "";
  } else {
    setChip(chip, "未安装", "warn");
    path.textContent = "未找到 repotory-mcp —— 安装 Repotory 后即可接入记忆层";
  }

  for (const tool of status.memory.tools) {
    const tag = document.createElement("span");
    tag.className = "tool";
    tag.textContent = tool;
    tools.appendChild(tag);
  }
}

function renderBackends(status: ShellStatus) {
  const list = pick<HTMLElement>("#backends");
  const defaultChip = pick<HTMLElement>("#default-chip");

  for (const backend of status.backends) {
    const item = document.createElement("li");
    item.className = "backend";

    const name = document.createElement("span");
    name.className = "backend__name";
    name.textContent = BACKEND_LABELS[backend.id] ?? backend.id;

    const chip = document.createElement("span");
    chip.className = "chip";
    if (backend.state === "planned") {
      setChip(chip, "规划中", "quiet");
    } else if (backend.available) {
      setChip(chip, "可用", "ok");
    } else {
      setChip(chip, "未安装", "warn");
    }

    item.append(name, chip);

    if (backend.path) {
      const path = document.createElement("span");
      path.className = "backend__path";
      path.textContent = backend.path;
      item.appendChild(path);
    }

    list.appendChild(item);
  }

  const isDefaultReady = status.backends.some(
    (b) => b.id === status.defaultBackend && b.available,
  );
  setChip(defaultChip, `默认 ${status.defaultModel}`, isDefaultReady ? "ok" : "warn");
}

async function main() {
  try {
    const status = await invoke<ShellStatus>("shell_status");
    renderMemory(status);
    renderBackends(status);
  } catch (error) {
    // The frontend also runs in a plain browser (npm run dev without Tauri),
    // where there is no backend to ask. Say that instead of showing an empty
    // screen that reads like a broken install.
    const detail = error instanceof Error ? error.message : String(error);
    setChip(pick<HTMLElement>("#memory-chip"), "后端不可达", "warn");
    pick<HTMLElement>("#memory-path").textContent = detail;
  }
}

main();
