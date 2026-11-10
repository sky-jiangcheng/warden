// Warden — independent startup lanes and the existing settings controls.
// Conversation state lives in chat.ts; the small desktop surface lives in
// companion.ts. Runtime/provider/tool knowledge still comes only from Rust.
// Stored API keys never return to the UI: a newly typed secret is passed
// directly to the backend, which writes it to a 0600 file beside the provider
// config. It is never sent back — `hasApiKey` is all the UI ever learns.
import { listen } from "@tauri-apps/api/event";
import { browserNote, desktop, errorText, initSheets, invoke, pick } from "./ui";
import { initChat } from "./chat";
import { initCompanion } from "./companion";

let chat: ReturnType<typeof initChat> | undefined;

type Provider = {
  id: string;
  name: string;
  baseUrl: string;
  defaultModel: string | null;
  headers: Record<string, string>;
  hasApiKey: boolean;
  active: boolean;
};

type ProviderList = {
  providers: Provider[];
  activeProviderId: string | null;
  problem: string | null;
};

type ModelInfo = {
  id: string;
  name: string | null;
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
  /// Contract version from initialize.serverInfo.version, when reported.
  version?: string;
};

type TrayStatus = {
  installed: boolean;
};

/// One thing the user can adopt as a project. A project may be a single
/// checkout or the folder directly above one — both are just a path here.
type ProjectCandidate = {
  path: string;
  name: string;
  kind: "repo" | "parent" | "missing";
  children: number;
  favorite: boolean;
};

/// Everything the repository screen draws, in one payload.
type RepoScan = {
  roots: string[];
  repos: RepoEntry[];
  newRepos: RepoEntry[];
  projects: ProjectCandidate[];
  truncated: boolean;
  problem: string | null;
};

type RepoEntry = {
  path: string;
  name: string;
  favorite: boolean;
  missing: boolean;
};

type ShellStatus = {
  providers: ProviderList;
  memory: MemoryStatus;
  tray: TrayStatus;
};

type ConnectionTest = {
  ok: boolean;
  detail: string;
  model: string;
  latencyMs: number;
};

function setChip(el: HTMLElement, text: string, variant: "ok" | "warn" | "quiet") {
  el.textContent = text;
  el.classList.remove("chip--ok", "chip--warn", "chip--quiet");
  el.classList.add(`chip--${variant}`);
}

/// The three-state theme preference (ADR-0050 §⑥): follow the OS, or pin
/// light / dark. The stylesheet keys off data-theme on <html>, so "system" is
/// resolved here rather than in CSS — one place decides, and the pre-paint
/// script in index.html mirrors the same resolution to avoid a flash.
type ThemePref = "system" | "light" | "dark";
const THEME_KEY = "warden-theme";
const THEME_LABEL: Record<ThemePref, string> = {
  system: "跟随系统",
  light: "浅色",
  dark: "深色",
};

function readThemePref(): ThemePref {
  try {
    const stored = localStorage.getItem(THEME_KEY);
    if (stored === "light" || stored === "dark" || stored === "system") return stored;
  } catch {
    /* private mode: fall through to the default */
  }
  return "system";
}

function applyThemePref(pref: ThemePref): void {
  const dark =
    pref === "dark" ||
    (pref === "system" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.setAttribute("data-theme", dark ? "dark" : "light");
}

/// Wire the three-state control: clicking cycles system → light → dark, and
/// a system preference keeps tracking the OS until it is pinned.
function initThemeToggle(): void {
  const button = pick<HTMLButtonElement>("#theme-toggle");
  let pref = readThemePref();

  const sync = (): void => {
    button.textContent = THEME_LABEL[pref];
    button.setAttribute("aria-label", `颜色外观：${THEME_LABEL[pref]}，点击切换`);
    applyThemePref(pref);
  };

  button.addEventListener("click", () => {
    pref = pref === "system" ? "light" : pref === "light" ? "dark" : "system";
    try {
      localStorage.setItem(THEME_KEY, pref);
    } catch {
      /* private mode: the choice simply does not outlive the session */
    }
    sync();
  });

  // While following the OS, an OS-level switch has to move the app too; a
  // pinned preference deliberately ignores it.
  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => {
      if (pref === "system") applyThemePref(pref);
    });

  window.addEventListener("storage", (event) => {
    if (event.key === THEME_KEY) { pref = readThemePref(); sync(); }
  });
  sync();
}

/// The state the editor keeps while it is open.
///
/// A new provider has no id yet — the Rust side derives one from the name
/// when it is saved.
let editingProviderId: string | null = null;

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
  const retry = pick<HTMLButtonElement>("#memory-retry");
  const toolsList = pick<HTMLElement>("#memory-tools");
  const versionLine = pick<HTMLElement>("#memory-version");
  const banner = pick<HTMLElement>("#memory-banner");
  const bannerText = pick<HTMLElement>("#memory-banner-text");
  const bannerRetry = pick<HTMLButtonElement>("#memory-banner-retry");
  const memory = status.memory;

  error.textContent = "";
  retry.hidden = true;
  bannerRetry.hidden = true;
  bannerText.textContent = "";
  banner.hidden = true;
  toolsList.replaceChildren();
  versionLine.textContent = "";

  if (!memory.available) {
    setChip(chip, "未安装", "warn");
    path.textContent = "未找到 repotory-mcp —— 装上它即可接入记忆层（桌面 App 非必需）";
    retry.hidden = false;
    banner.hidden = false;
    bannerText.textContent = "本次对话没有记忆：本机未找到记忆层，装上它才有项目上下文依据。";
    return;
  }

  path.textContent = memory.path ?? "";

  if (!memory.connected) {
    setChip(chip, "连不上", "warn");
    error.textContent = memory.error ?? "记忆层没有应答";
    // The server may have been installed a moment ago; a retry is cheaper
    // than telling the user to restart the app.
    retry.hidden = false;
    banner.hidden = false;
    bannerRetry.hidden = false;
    bannerText.textContent = "本次对话没有上下文依据：记忆层连不上，聊天照常可用，但模型这次没有依据。";
    return;
  }

  // The names and descriptions are the server's own list, not a local copy.
  for (const tool of memory.tools) {
    const item = document.createElement("li");
    item.textContent = tool.name;
    if (tool.description) item.title = tool.description;
    toolsList.appendChild(item);
  }
  const version = memory.version;
  // ADR-0048: the server reports the contract version. Warden keeps no copy
  // and hard-codes no floor, so one reported generation has nothing to compare
  // against: show the generation and skip the "too old" warning rather than
  // inventing a magic number.
  const generation = version?.match(/^v?(\d+)/)?.[1] ?? null;
  setChip(chip, version ? `记忆层 v${version} · ${memory.tools.length} 工具` : `${memory.tools.length} 工具`, "ok");
  versionLine.textContent = generation !== null
    ? `契约代 ${generation} · 版本与工具清单均由服务端上报，本仓不抄一份`
    : "契约版本未上报，本仓不猜测";
}

/// The latest scan payload. The checkbox list is built from it only when the
/// page is safe to refresh; saving reads the DOM, which is the user's answer.
let lastScan: RepoScan | null = null;

/// A checkbox whose checked state differs from its rendered default is an
/// un-saved tick: rebuilding the list would clear it.
function unsavedSelection(): boolean {
  return [...document.querySelectorAll<HTMLInputElement>("#projects .project__box")]
    .some((box) => box.checked !== box.defaultChecked);
}

/// Show one page of the main window. The window itself never changes; only
/// which surface is visible. Used by the quiet header button, the settings
/// back button and the tray's navigation event — one switch, no drift.
type View = "chat" | "settings";
function showView(view: View): void {
  // Dialogs are body-level children; one left open would float over settings.
  for (const dialog of document.querySelectorAll<HTMLDialogElement>("dialog[open]")) dialog.close();
  document.documentElement.dataset.view = view;
  pick<HTMLElement>("#main-surface").hidden = view !== "chat";
  pick<HTMLElement>("#settings-view").hidden = view !== "settings";
  if (view === "chat") {
    // Leaving settings drops an editor left half-filled.
    pick<HTMLElement>("#provider-editor").hidden = true;
    pick<HTMLInputElement>("#field-api-key").value = "";
    editingProviderId = null;
    pick<HTMLTextAreaElement>("#chat-input").focus();
    return;
  }
  pick<HTMLElement>("#settings-heading").focus();
  if (!lastScan) return;
  // A rebuild would silently clear ticks the user has not saved yet.
  if (unsavedSelection()) {
    pick<HTMLElement>("#projects-note").textContent = "有未保存的更改 · 点「保存所选」提交。";
    return;
  }
  // A background scan only updated lastScan; draw it when nothing is unsaved.
  renderRepos(lastScan);
}

/// Render the scan roots and the selectable project list.
function renderRepos(scan: RepoScan, preserveSelection = false) {
  // A scan arriving while the user has ticks restores them by data-path.
  const kept = preserveSelection
    ? new Map([...document.querySelectorAll<HTMLInputElement>("#projects .project__box")].map((box) => [box.dataset.path ?? "", box.checked]))
    : null;
  lastScan = scan;
  // The chat context picker offers the projects the user adopted, not every
  // checkout the scanner happened to walk past.
  chat?.setRepos(scan.projects.map((project) => ({
    path: project.path,
    name: project.name,
    missing: project.kind === "missing",
  })));
  const chip = pick<HTMLElement>("#repo-chip");
  const list = pick<HTMLElement>("#projects");
  const roots = pick<HTMLElement>("#roots");
  const detail = pick<HTMLElement>("#repo-detail");
  const error = pick<HTMLElement>("#repo-error");
  const note = pick<HTMLElement>("#projects-note");
  const save = pick<HTMLButtonElement>("#repo-save-projects");

  list.replaceChildren();
  roots.replaceChildren();
  error.textContent = "";

  if (scan.problem) {
    setChip(chip, "配置有问题", "warn");
    error.textContent = scan.problem;
  }

  // --- the scan roots -------------------------------------------------

  if (scan.roots.length === 0) {
    const hint = document.createElement("li");
    hint.className = "roots__empty";
    hint.textContent = "还没加目录 —— 填一个你放代码的路径，比如 ~/work。";
    roots.appendChild(hint);
  } else {
    for (const root of scan.roots) {
      const item = document.createElement("li");
      item.className = "root";
      const path = document.createElement("span");
      path.className = "root__path";
      path.textContent = root;
      const actions = document.createElement("div");
      actions.className = "root__actions";
      actions.appendChild(makeButton("移除", () => {
        void invoke<RepoScan>("repo_roots_remove", { path: root }).then(
          (next) => renderRepos(next, true),
          (err: unknown) => { pick<HTMLElement>("#repo-error").textContent = String(err); },
        );
      }));
      item.append(path, actions);
      roots.appendChild(item);
    }
  }

  // --- the selectable projects ---------------------------------------

  // Nothing is announced. The result of a scan IS this list: repositories
  // and their parent folders side by side, each with one checkbox.
  if (scan.roots.length === 0) {
    const empty = document.createElement("li");
    empty.className = "roots__empty";
    empty.textContent = "先加一个目录，扫描结果会在这里出现。";
    list.appendChild(empty);
  } else if (scan.projects.length === 0) {
    const empty = document.createElement("li");
    empty.className = "roots__empty";
    empty.textContent = "这些目录里没有 git 仓库。";
    list.appendChild(empty);
  } else {
    for (const project of scan.projects) list.appendChild(renderProjectRow(project, kept?.get(project.path)));
  }

  const selected = scan.projects.filter((project) => project.favorite).length;
  save.disabled = scan.projects.length === 0;
  note.textContent = selected > 0
    ? `已选 ${selected} 个项目 · 聊天时可附加为上下文。`
    : "选中的项目会成为聊天时可附加的上下文。";
  setChip(chip, `${scan.projects.length} 个候选 · ${selected} 个已选`, selected > 0 ? "ok" : "quiet");
  detail.textContent = scan.truncated ? "扫到的仓库太多，只列了前一批 —— 把目录指得更准一点。" : "";
}

/// One checkbox row. A parent folder is labelled with how many checkouts it
/// holds, so "单仓库" and "仓库的上一级" are distinguishable at a glance.
function renderProjectRow(project: ProjectCandidate, checked?: boolean): HTMLLIElement {
  const item = document.createElement("li");
  item.className = "project";
  item.dataset.path = project.path;

  const box = document.createElement("input");
  box.type = "checkbox";
  box.className = "project__box";
  box.id = `project-${project.path.replace(/[^a-zA-Z0-9]/g, "-")}`;
  // defaultChecked records the saved baseline so un-saved ticks are visible.
  box.defaultChecked = project.favorite;
  box.checked = checked ?? project.favorite;
  box.dataset.path = project.path;

  const main = document.createElement("div");
  main.className = "project__main";
  const name = document.createElement("label");
  name.className = "project__name";
  name.htmlFor = box.id;
  name.textContent = project.name;
  const path = document.createElement("span");
  path.className = "project__path";
  path.textContent = project.path;
  path.title = project.path;
  main.append(name, path);

  const tags = document.createElement("div");
  tags.className = "project__tags";
  const kind = document.createElement("span");
  kind.className = `chip ${project.kind === "parent" ? "chip--ok" : "chip--quiet"}`;
  kind.textContent = project.kind === "parent"
    ? `目录 · ${project.children} 个仓库`
    : project.kind === "missing" ? "找不到" : "单仓库";
  tags.appendChild(kind);

  item.append(box, main, tags);
  return item;
}

/// Save the whole selection in one command. There is no one-at-a-time
/// question, no queue and no pop-up: the list is the configuration surface.
function saveProjects(): void {
  const paths = [...document.querySelectorAll<HTMLInputElement>("#projects .project__box")]
    .filter((box) => box.checked)
    .map((box) => box.dataset.path ?? "");
  void invoke<RepoScan>("repo_set_projects", { paths }).then(
    (scan) => {
      renderRepos(scan);
      pick<HTMLElement>("#repo-error").textContent = "";
    },
    (err: unknown) => { pick<HTMLElement>("#repo-error").textContent = String(err); },
  );
}

/// Load the repository screen. Never announces anything.
async function loadRepos() {
  try {
    renderRepos(await invoke<RepoScan>("repo_scan"), true);
  } catch (error) {
    pick<HTMLElement>("#repo-error").textContent =
      error instanceof Error ? error.message : String(error);
  }
}

/// A background scan publishes its result as data only. It never rebuilds the
/// checkbox list: that would discard un-saved selections while the user is
/// looking at it. showView draws lastScan when the page is safe to refresh.
async function watchForRepoChanges() {
  try {
    await listen<RepoScan>("repos:changed", (event) => {
      lastScan = event.payload;
    });
  } catch {
    // No backend (the frontend also runs in a plain browser).
  }
}

/// Render the provider list.
///
/// The list is rebuilt from scratch on every render: a save re-invokes the
/// list command, and appending to a stale list would double every row.
function renderProviders(list: ProviderList) {
  chat?.setProviders(list.providers);

  const chip = pick<HTMLElement>("#provider-chip");
  const container = pick<HTMLElement>("#providers");
  const error = pick<HTMLElement>("#provider-error");

  container.replaceChildren();
  error.textContent = "";

  if (list.problem) {
    setChip(chip, "配置有问题", "warn");
    error.textContent = list.problem;
  } else if (list.providers.length === 0) {
    setChip(chip, "未配置", "warn");
  } else {
    const active = list.providers.find((p) => p.active);
    setChip(
      chip,
      active ? `${active.name} · ${active.defaultModel ?? "未选模型"}` : "未选中",
      active ? "ok" : "warn",
    );
  }

  for (const provider of list.providers) {
    const item = document.createElement("li");
    item.className = "provider";

    const name = document.createElement("span");
    name.className = "provider__name";
    name.textContent = provider.name;

    const url = document.createElement("span");
    url.className = "provider__url";
    url.textContent = provider.baseUrl;

    const row = document.createElement("div");
    row.className = "provider__row";
    row.append(name, url);

    const tags = document.createElement("div");
    tags.className = "provider__tags";

    if (provider.active) {
      const activeTag = document.createElement("span");
      activeTag.className = "chip chip--ok";
      activeTag.textContent = "使用中";
      tags.appendChild(activeTag);
    }
    if (provider.hasApiKey) {
      const keyTag = document.createElement("span");
      keyTag.className = "chip chip--quiet";
      keyTag.textContent = "有 Key";
      tags.appendChild(keyTag);
    }

    const actions = document.createElement("div");
    actions.className = "provider__actions-inline";

    if (!provider.active) {
      actions.appendChild(
        makeButton("设为使用中", () => {
          void invoke<ProviderList>("provider_set_active", { id: provider.id }).then(
            (list: ProviderList) => renderProviders(list),
            (err: unknown) => {
              pick<HTMLElement>("#provider-error").textContent = String(err);
            },
          );
        }),
      );
    }
    actions.appendChild(
      makeButton("编辑", () => openEditor(provider)),
    );
    actions.appendChild(
      makeButton("删除", () => {
        void invoke<ProviderList>("provider_remove", { id: provider.id }).then(
          (list: ProviderList) => renderProviders(list),
          (err: unknown) => {
            pick<HTMLElement>("#provider-error").textContent = String(err);
          },
        );
      }),
    );

    row.appendChild(tags);
    row.appendChild(actions);
    item.appendChild(row);
    container.appendChild(item);
  }

}

/// A small text button.
function makeButton(label: string, onClick: () => void): HTMLButtonElement {
  const button = document.createElement("button");
  button.className = "btn btn--tight";
  button.type = "button";
  button.textContent = label;
  button.addEventListener("click", onClick);
  return button;
}

/// Open the editor for a provider, or for a new one.
function openEditor(provider?: Provider) {
  const editor = pick<HTMLElement>("#provider-editor");
  editor.hidden = false;

  editingProviderId = provider?.id ?? null;
  pick<HTMLElement>("#editor-title").textContent = provider
    ? `编辑 ${provider.name}`
    : "添加 provider";
  pick<HTMLInputElement>("#field-name").value = provider?.name ?? "";
  pick<HTMLInputElement>("#field-base-url").value = provider?.baseUrl ?? "";
  pick<HTMLInputElement>("#field-model").value = provider?.defaultModel ?? "";
  // The stored key is never sent to this window, so the field starts empty.
  // Leaving it empty keeps the stored key; a new value replaces it.
  pick<HTMLInputElement>("#field-api-key").value = "";
  pick<HTMLElement>("#editor-error").textContent = "";
  pick<HTMLElement>("#editor-detail").textContent = provider?.hasApiKey
    ? "已存有 API Key —— 留空则保持不变"
    : "";
  pick<HTMLInputElement>("#field-name").focus();
}

/// The values the editor currently holds.
function editorValues() {
  return {
    id: editingProviderId ?? "",
    name: pick<HTMLInputElement>("#field-name").value.trim(),
    baseUrl: pick<HTMLInputElement>("#field-base-url").value.trim(),
    apiKey: pick<HTMLInputElement>("#field-api-key").value,
    defaultModel: pick<HTMLInputElement>("#field-model").value.trim(),
  };
}

/// Why the editor cannot probe yet, or null when it can.
///
/// A NEW provider has nothing saved to fall back on, so without a Base URL
/// there is no endpoint to ask. Saying that here beats letting the backend
/// answer "no provider is configured yet", which points at the wrong thing:
/// the user is IN the add-provider form, and the one thing missing is the
/// URL they have to type.
function editorProbeBlocked(values: ReturnType<typeof editorValues>): string | null {
  if (!values.id && !values.baseUrl) {
    return "请先填写 Base URL —— 模型列表与连接测试都按它去请求";
  }
  return null;
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
    note.textContent = "⌥ Space 或点菜单栏标记唤起聊天；右键可进设置、显隐桌宠、退出 Warden。";
  } else {
    setChip(chip, "不可用", "warn");
    note.textContent =
      "托盘未就位。桌宠仍可点开对话；日志中可查看 [tray] 错误。";
  }
}

function main() {
  initThemeToggle();
  if (new URLSearchParams(location.search).get("surface") === "companion") {
    initCompanion();
    return;
  }
  initSheets();
  chat = initChat();
  // The "new repository" bubble stays a top-level surface. Moving it into the
  // repository sheet (a closed <dialog>) meant the question never appeared
  // while the user was in the conversation — which is exactly where it belongs.
  for (const error of document.querySelectorAll<HTMLElement>(".meta--error")) error.setAttribute("role", "alert");

  let refreshing = false;
  const refresh = async () => {
    if (refreshing) return;
    refreshing = true;
    const retry = pick<HTMLButtonElement>("#memory-retry");
    retry.disabled = true;
    try {
      const status = await invoke<ShellStatus>("shell_status");
      renderMemory(status);
      renderTray(status);
      chat?.retrySync();
    } catch (error) {
      setChip(pick<HTMLElement>("#memory-chip"), "未连接", "warn");
      pick<HTMLElement>("#memory-error").textContent = errorText(error);
      pick<HTMLElement>("#memory-banner").hidden = false;
      pick<HTMLElement>("#memory-banner-text").textContent = "本次对话没有上下文依据：记忆层状态读取失败，聊天照常可用。";
      pick<HTMLButtonElement>("#memory-banner-retry").hidden = false;
    } finally {
      refreshing = false;
      retry.hidden = false;
      retry.disabled = !desktop;
    }
  };
  let loadingProviders = false;
  const loadProviders = async () => {
    if (loadingProviders) return;
    loadingProviders = true;
    try { renderProviders(await invoke<ProviderList>("provider_list")); }
    catch (error) {
      setChip(pick<HTMLElement>("#provider-chip"), "未连接", "warn");
      pick<HTMLElement>("#provider-error").textContent = errorText(error);
    } finally { loadingProviders = false; }
  };
  pick<HTMLButtonElement>("#provider-retry").addEventListener("click", () => { void loadProviders(); });
  // Independent lanes: a slow optional MCP handshake cannot delay models,
  // repository discovery, or the already-painted conversation surface.
  if (desktop) {
    void refresh();
    void loadProviders();
    void watchForRepoChanges();
    void loadRepos();
  } else {
    for (const id of ["#memory-chip", "#provider-chip", "#repo-chip", "#tray-chip"]) setChip(pick<HTMLElement>(id), "未连接", "quiet");
    pick<HTMLElement>("#memory-path").textContent = browserNote;
    pick<HTMLElement>("#tray-path").textContent = "浏览器预览没有托盘与快捷键；设置页仍可切换。";
    for (const id of ["#root-add", "#repo-scan", "#repo-save-projects", "#provider-save", "#provider-fetch-models", "#provider-test", "#provider-retry", "#memory-retry"]) pick<HTMLButtonElement>(id).disabled = true;
  }

  // --- page switching --------------------------------------------------

  showView("chat");
  pick<HTMLButtonElement>("#settings-open").addEventListener("click", () => showView("settings"));
  pick<HTMLButtonElement>("#settings-back").addEventListener("click", () => showView("chat"));
  pick<HTMLElement>("#settings-view").addEventListener("keydown", (event) => {
    if (event.key === "Escape") showView("chat");
  });
  // The tray's Settings item asks the backend to show this window; the
  // backend emits the view to display. One switch, two entry points.
  void listen<string>("warden:navigate", (event) => {
    showView(event.payload === "settings" ? "settings" : "chat");
  }).catch(() => { /* plain browser: no backend events */ });
  // Rust emits window:error when hiding a surface fails; show it plainly.
  void listen<string>("window:error", (event) => {
    pick<HTMLElement>("#chat-error").textContent = errorText(event.payload);
  }).catch(() => { /* plain browser: no backend events */ });
  // Reuse the existing page switch and retry button; no new event is created.
  pick<HTMLButtonElement>("#memory-banner-settings").addEventListener("click", () => showView("settings"));
  pick<HTMLButtonElement>("#memory-banner-retry").addEventListener("click", () => pick<HTMLButtonElement>("#memory-retry").click());

  pick<HTMLFormElement>("#root-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const input = pick<HTMLInputElement>("#root-input");
    const button = pick<HTMLButtonElement>("#root-add");
    const path = input.value.trim();
    if (!path || button.disabled) return;
    button.disabled = true;
    // Configuring a root IS the first scan. Its result appears as the
    // selectable project list right here — no pop-up, no question queue.
    void invoke<RepoScan>("repo_roots_add", { path }).then(
      (scan: RepoScan) => {
        if (input.value.trim() === path) input.value = "";
        renderRepos(scan, true);
      },
      (err: unknown) => { pick<HTMLElement>("#repo-error").textContent = errorText(err); },
    ).finally(() => { button.disabled = false; });
  });

  pick<HTMLButtonElement>("#repo-scan").addEventListener("click", () => { void loadRepos(); });
  pick<HTMLButtonElement>("#repo-save-projects").addEventListener("click", saveProjects);

  // A failed connection is worth one more try: the server may have been
  // installed since the window opened. The retry re-runs the same status
  // command — the Rust side owns whether that means reconnecting.
  pick<HTMLButtonElement>("#memory-retry").addEventListener("click", () => {
    void refresh().catch((error: unknown) => {
      const detail = error instanceof Error ? error.message : String(error);
      pick<HTMLElement>("#memory-error").textContent = detail;
    });
  });

  // --- the provider editor ---------------------------------------------

  pick<HTMLButtonElement>("#provider-add").addEventListener("click", () => {
    openEditor();
  });

  pick<HTMLButtonElement>("#provider-cancel").addEventListener("click", () => {
    pick<HTMLElement>("#provider-editor").hidden = true;
    pick<HTMLInputElement>("#field-api-key").value = "";
    pick<HTMLButtonElement>("#provider-add").focus();
  });

  pick<HTMLFormElement>("#provider-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const values = editorValues();
    const editorError = pick<HTMLElement>("#editor-error");
    const editorDetail = pick<HTMLElement>("#editor-detail");
    editorError.textContent = "";
    editorDetail.textContent = "";

    void invoke<ProviderList>("provider_save", {
      input: {
        id: values.id,
        name: values.name,
        baseUrl: values.baseUrl,
        defaultModel: values.defaultModel || null,
        apiKey: values.apiKey || null,
        // The key is only written when one was typed. An empty field on an
        // existing provider means "keep the stored one".
        saveApiKey: values.apiKey.length > 0,
      },
    }).then(
      (list: ProviderList) => {
        pick<HTMLElement>("#provider-editor").hidden = true;
        pick<HTMLInputElement>("#field-api-key").value = "";
        renderProviders(list);
        pick<HTMLButtonElement>("#provider-add").focus();
      },
      (err: unknown) => {
        editorError.textContent = errorText(err);
      },
    );
  });

  pick<HTMLButtonElement>("#provider-fetch-models").addEventListener("click", () => {
    const values = editorValues();
    const editorError = pick<HTMLElement>("#editor-error");
    const editorDetail = pick<HTMLElement>("#editor-detail");
    editorError.textContent = "";
    const blocked = editorProbeBlocked(values);
    if (blocked) {
      editorDetail.textContent = "";
      editorError.textContent = blocked;
      return;
    }
    editorDetail.textContent = "拉取中…";

    void invoke<ModelInfo[]>("provider_fetch_models", {
      baseUrl: values.baseUrl || null,
      id: values.id || null,
      apiKey: values.apiKey || null,
    }).then(
      (models: ModelInfo[]) => {
        const options = pick<HTMLDataListElement>("#model-options");
        options.replaceChildren();
        for (const model of models) {
          const option = document.createElement("option");
          option.value = model.id;
          options.appendChild(option);
        }
        editorDetail.textContent = `拉到 ${models.length} 个模型`;
        // The first model is a starting point, not a choice: it is filled in
        // only when the field is still empty.
        if (!values.defaultModel && models.length > 0) {
          pick<HTMLInputElement>("#field-model").value = models[0].id;
        }
      },
      (err: unknown) => {
        editorDetail.textContent = "";
        editorError.textContent = err instanceof Error ? err.message : String(err);
      },
    );
  });

  pick<HTMLButtonElement>("#provider-test").addEventListener("click", () => {
    const values = editorValues();
    const editorError = pick<HTMLElement>("#editor-error");
    const editorDetail = pick<HTMLElement>("#editor-detail");
    editorError.textContent = "";
    const blocked = editorProbeBlocked(values);
    if (blocked) {
      editorDetail.textContent = "";
      editorError.textContent = blocked;
      return;
    }
    editorDetail.textContent = "测试中…";

    void invoke<ConnectionTest>("provider_test_connection", {
      baseUrl: values.baseUrl || null,
      id: values.id || null,
      apiKey: values.apiKey || null,
      model: values.defaultModel || null,
    }).then(
      (result: ConnectionTest) => {
        if (result.ok) {
          editorDetail.textContent = `连接正常 · ${result.model} · ${result.latencyMs}ms · 回答「${result.detail}」`;
        } else {
          editorDetail.textContent = "";
          editorError.textContent = result.detail;
        }
      },
      (err: unknown) => {
        editorDetail.textContent = "";
        editorError.textContent = err instanceof Error ? err.message : String(err);
      },
    );
  });

}

main();
