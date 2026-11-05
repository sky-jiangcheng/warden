// Repotory Agent — first screen.
//
// The window asks the Rust side for one status payload and renders it. The
// frontend owns no runtime knowledge: which model backends exist, and whether
// the Repotory MCP server is installed, are both facts the backend owns.
//
// The memory layer's tool list is the sharpest case of that rule. The server
// advertises its tools over MCP's `tools/list`; the Rust client fetches that
// list over the wire; this file draws whatever arrived. No tool name is
// written down here — a copy would drift, and a drifted copy reads like a
// contract.
import { invoke } from "@tauri-apps/api/core";

type BackendStatus = {
  id: string;
  available: boolean;
  path: string | null;
  state: string;
};

/// One tool the MCP server advertised.
type MemoryTool = {
  name: string;
  description: string;
};

type MemoryStatus = {
  /// The server binary is on this machine.
  available: boolean;
  path: string | null;
  /// The server answered the handshake and gave its tool list.
  connected: boolean;
  /// The tools the server advertised. Empty means "we do not know", never
  /// "the server has none".
  tools: MemoryTool[];
  /// Why the server could not be reached, when it is installed but silent.
  error: string | null;
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

/// Render the memory layer's state.
///
/// The three states are kept apart because they mean different things to the
/// person looking at the screen, and telling them apart is the whole point:
/// "not installed" is an install problem, "installed but not answering" is a
/// version or environment problem, and "connected" is the only state in which
/// the copilot actually has a memory. A screen that reports "installed" while
/// the server refuses to answer is lying about the one thing that matters.
function renderMemory(status: ShellStatus) {
  const chip = pick<HTMLElement>("#memory-chip");
  const path = pick<HTMLElement>("#memory-path");
  const error = pick<HTMLElement>("#memory-error");
  const tools = pick<HTMLElement>("#memory-tools");
  const retry = pick<HTMLButtonElement>("#memory-retry");

  tools.replaceChildren();
  error.textContent = "";
  retry.hidden = true;

  if (!status.memory.available) {
    setChip(chip, "未安装", "warn");
    path.textContent = "未找到 repotory-mcp —— 安装 Repotory 后即可接入记忆层";
    return;
  }

  path.textContent = status.memory.path ?? "";

  if (!status.memory.connected) {
    setChip(chip, "连不上", "warn");
    error.textContent = status.memory.error ?? "记忆层没有应答";
    // The server may have been installed a moment ago; a retry is cheaper
    // than telling the user to restart the app.
    retry.hidden = false;
    return;
  }

  setChip(chip, `已连接 · ${status.memory.tools.length} 个工具`, "ok");
  for (const tool of status.memory.tools) {
    const item = document.createElement("li");
    item.className = "tool";

    const name = document.createElement("span");
    name.className = "tool__name";
    name.textContent = tool.name;

    const description = document.createElement("span");
    description.className = "tool__desc";
    description.textContent = tool.description;
    // The card clamps the description to three lines; the title keeps the
    // server's full wording one hover away.
    description.title = tool.description;

    item.append(name, description);
    tools.appendChild(item);
  }
}

function renderBackends(status: ShellStatus) {
  const list = pick<HTMLElement>("#backends");
  const defaultChip = pick<HTMLElement>("#default-chip");

  // The list is rebuilt from scratch on every render: a retry re-invokes the
  // status command, and appending to a stale list would double every row.
  list.replaceChildren();

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
  const refresh = async () => {
    const status = await invoke<ShellStatus>("shell_status");
    renderMemory(status);
    renderBackends(status);
    renderTray(status);
  };

  try {
    await refresh();
  } catch (error) {
    // The frontend also runs in a plain browser (npm run dev without Tauri),
    // where there is no backend to ask. Say that instead of showing an empty
    // screen that reads like a broken install.
    const detail = error instanceof Error ? error.message : String(error);
    setChip(pick<HTMLElement>("#memory-chip"), "后端不可达", "warn");
    pick<HTMLElement>("#memory-path").textContent = detail;
  }

  // A failed connection is worth one more try: the server may have been
  // installed since the window opened. The retry re-runs the same status
  // command — the Rust side owns whether that means reconnecting.
  pick<HTMLButtonElement>("#memory-retry").addEventListener("click", () => {
    void refresh().catch((error: unknown) => {
      const detail = error instanceof Error ? error.message : String(error);
      pick<HTMLElement>("#memory-error").textContent = detail;
    });
  });
}

main();
