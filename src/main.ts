// Repotory Agent — first screen.
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
};

type TrayStatus = {
  installed: boolean;
};

type ShellStatus = {
  backends: BackendStatus[];
  defaultBackend: string;
  defaultModel: string;
  memory: MemoryStatus;
  tray: TrayStatus;
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

  if (status.memory.available) {
    setChip(chip, "已安装", "ok");
    path.textContent = status.memory.path ?? "";
  } else {
    setChip(chip, "未安装", "warn");
    path.textContent = "未找到 repotory-mcp —— 安装 Repotory 后即可接入记忆层";
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

/// Render the tray (the wake-up entry) state.
///
/// The tray is reported rather than assumed: macOS gives no way to check
/// another process's status items from the outside, so the app can only
/// answer for itself. A user who cannot find the tray needs to be able to
/// tell "it failed to install" from "it is there and I missed it" — and the
/// first screen is where that distinction has to be visible.
function renderTray(status: ShellStatus) {
  const chip = pick<HTMLElement>("#tray-chip");
  const note = pick<HTMLElement>("#tray-path");

  if (status.tray.installed) {
    setChip(chip, "已就位", "ok");
    note.textContent = "点菜单栏的 R 唤回窗口，右键打开菜单。";
  } else {
    setChip(chip, "不可用", "warn");
    note.textContent =
      "托盘图标没能装上——这个窗口关掉就找不回来了。日志里有 [tray] 开头的错误。";
  }
}

async function main() {
  try {
    const status = await invoke<ShellStatus>("shell_status");
    renderMemory(status);
    renderBackends(status);
    renderTray(status);
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
