// Repotory Agent — first screen.
//
// The window asks the Rust side for one status payload and renders it. The
// frontend owns no runtime knowledge: which model providers exist, and
// whether the Repotory MCP server is installed, are both facts the backend
// owns.
//
// The two lists this screen draws are both fetched, never written down here:
//
//   * the memory layer's tool list comes over MCP's `tools/list`;
//   * a provider's model list comes over the OpenAI-compatible `/models`.
//
// A copy would drift, and a drifted copy reads like a contract.
//
// The API key never reaches this file. It is typed into a password field,
// handed straight to the Rust side, and stored in the operating system's
// credential store — this window only ever learns WHETHER a key exists.
import { invoke } from "@tauri-apps/api/core";
import { Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
};

type TrayStatus = {
  installed: boolean;
};

/// One repository the copilot found on this machine.
type RepoEntry = {
  path: string;
  name: string;
  favorite: boolean;
  missing: boolean;
};

/// Everything the repository screen draws, in one payload.
type RepoScan = {
  roots: string[];
  repos: RepoEntry[];
  newRepos: RepoEntry[];
  truncated: boolean;
  problem: string | null;
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

/// One streamed piece of an answer.
type ChatChunk = {
  delta: string;
  text: string;
  done: boolean;
  error: string | null;
};

/// The one element with this selector, or an error.
///
/// The error is the point: a missing element means the screen and the code
/// disagree about what exists, and that is a bug worth hearing about rather
/// than a null to guard against. For an element that is legitimately
/// optional, use `querySelector` directly.
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

/// The state the editor keeps while it is open.
///
/// A new provider has no id yet — the Rust side derives one from the name
/// when it is saved.
let editingProviderId: string | null = null;

/// The provider list as the backend last reported it.
let currentProviders: Provider[] = [];

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

/// Repositories the user has not answered for yet.
///
/// The queue is what keeps the toast honest: one repository at a time, and
/// a repository that is already waiting is never announced twice.
let pendingRepos: RepoEntry[] = [];

/// The repository the toast is asking about right now.
let currentRepo: RepoEntry | null = null;

/// Render the scan roots and the repository list.
function renderRepos(scan: RepoScan) {
  const chip = pick<HTMLElement>("#repo-chip");
  const list = pick<HTMLElement>("#repos");
  const roots = pick<HTMLElement>("#roots");
  const detail = pick<HTMLElement>("#repo-detail");
  const error = pick<HTMLElement>("#repo-error");

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
    hint.textContent =
      "还没加目录 —— 填一个你放代码的路径，比如 ~/work。";
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
      actions.appendChild(
        makeButton("移除", () => {
          void invoke<RepoScan>("repo_roots_remove", { path: root }).then(
            (scan: RepoScan) => renderRepos(scan),
            (err: unknown) => {
              pick<HTMLElement>("#repo-error").textContent = String(err);
            },
          );
        }),
      );

      item.append(path, actions);
      roots.appendChild(item);
    }
  }

  // --- the repositories ------------------------------------------------

  // Followed repositories first: they are the ones the user has already
  // answered for, and they are what the copilot will act on.
  const followed = scan.repos.filter((repo) => repo.favorite);
  const others = scan.repos.filter((repo) => !repo.favorite);

  if (scan.repos.length === 0) {
    const empty = document.createElement("li");
    empty.className = "repos__empty";
    empty.textContent =
      scan.roots.length === 0
        ? "先加一个目录，copilot 才知道去哪儿找。"
        : "这些目录里没有 git 仓库。";
    list.appendChild(empty);
  } else {
    for (const repo of [...followed, ...others]) {
      list.appendChild(renderRepoRow(repo));
    }
  }

  setChip(
    chip,
    followed.length > 0
      ? `${scan.repos.length} 个仓库 · ${followed.length} 个已收藏`
      : `${scan.repos.length} 个仓库`,
    followed.length > 0 ? "ok" : "quiet",
  );

  detail.textContent = scan.truncated
    ? "扫到的仓库太多，只列了前一批 —— 把目录指得更准一点。"
    : "";
}

/// One row of the repository list.
function renderRepoRow(repo: RepoEntry): HTMLLIElement {
  const item = document.createElement("li");
  item.className = "repo";

  const main = document.createElement("div");
  main.className = "repo__main";

  const name = document.createElement("span");
  name.className = "repo__name";
  name.textContent = repo.name;

  const path = document.createElement("span");
  path.className = "repo__path";
  path.textContent = repo.path;
  path.title = repo.path;

  main.append(name, path);

  const tags = document.createElement("div");
  tags.className = "repo__tags";

  if (repo.favorite) {
    const tag = document.createElement("span");
    tag.className = "chip chip--ok";
    tag.textContent = "已收藏";
    tags.appendChild(tag);
  }
  if (repo.missing) {
    const tag = document.createElement("span");
    tag.className = "chip chip--warn";
    tag.textContent = "找不到";
    tags.appendChild(tag);
  }

  const actions = document.createElement("div");
  actions.className = "repo__actions";
  actions.appendChild(
    makeButton(repo.favorite ? "取消收藏" : "收藏", () => {
      void invoke<RepoScan>("repo_set_favorite", {
        path: repo.path,
        favorite: !repo.favorite,
      }).then(
        (scan: RepoScan) => renderRepos(scan),
        (err: unknown) => {
          pick<HTMLElement>("#repo-error").textContent = String(err);
        },
      );
    }),
  );

  item.append(main, tags, actions);
  return item;
}

/// Ask about the next repository the user has not answered for.
///
/// One at a time. Five "is this yours?" cards stacked at once is a form,
/// not a question — and the answer only means something when it is about
/// one repository.
function showNextRepoToast() {
  if (currentRepo || pendingRepos.length === 0) return;

  const repo = pendingRepos.shift();
  if (!repo) return;
  currentRepo = repo;

  pick<HTMLElement>("#toast-name").textContent = repo.name;
  const path = pick<HTMLElement>("#toast-path");
  path.textContent = repo.path;
  // The path is clipped to keep the bubble narrow, so the full one has to
  // be one hover away.
  path.title = repo.path;

  const toast = pick<HTMLElement>("#repo-toast");
  toast.hidden = false;
  // The browser has to paint the un-hidden state before the transition
  // runs, otherwise the slide-in has nothing to animate from.
  requestAnimationFrame(() => toast.classList.add("toast--in"));
}

/// Put the toast away and let the next one in.
function hideRepoToast() {
  const toast = pick<HTMLElement>("#repo-toast");
  toast.classList.remove("toast--in");
  toast.hidden = true;
  currentRepo = null;
}

/// Answer the question the toast asks.
function answerToast(favorite: boolean) {
  const repo = currentRepo;
  if (!repo) return;
  hideRepoToast();

  const done = (scan: RepoScan) => {
    renderRepos(scan);
    showNextRepoToast();
  };
  const failed = (err: unknown) => {
    pick<HTMLElement>("#repo-error").textContent = String(err);
  };

  if (favorite) {
    void invoke<RepoScan>("repo_set_favorite", {
      path: repo.path,
      favorite: true,
    }).then(done, failed);
  } else {
    void invoke<RepoScan>("repo_dismiss", { path: repo.path }).then(done, failed);
  }
}

/// Queue the repositories the user has not dealt with, and ask about the
/// first one.
function queueNewRepos(repos: RepoEntry[]) {
  for (const repo of repos) {
    const waiting = pendingRepos.some((queued) => queued.path === repo.path);
    if (!waiting && currentRepo?.path !== repo.path) {
      pendingRepos.push(repo);
    }
  }
  showNextRepoToast();
}

/// Load the repository screen.
async function loadRepos() {
  try {
    const scan = await invoke<RepoScan>("repo_scan");
    renderRepos(scan);
    queueNewRepos(scan.newRepos);
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    pick<HTMLElement>("#repo-error").textContent = detail;
  }
}

/// Watch for repositories found while the window is open.
async function watchForNewRepos() {
  try {
    await listen<RepoEntry[]>("repos:new", (event) => {
      queueNewRepos(event.payload);
    });
  } catch {
    // No backend (the frontend also runs in a plain browser). There is
    // nothing to listen to, and that is not a failure worth reporting.
  }
}

/// Render the provider list.
///
/// The list is rebuilt from scratch on every render: a save re-invokes the
/// list command, and appending to a stale list would double every row.
function renderProviders(list: ProviderList) {
  currentProviders = list.providers;

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

  renderModelPickers();
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

/// Fill the provider and model pickers in the chat form.
function renderModelPickers() {
  const providerSelect = pick<HTMLSelectElement>("#chat-provider");
  const modelSelect = pick<HTMLSelectElement>("#chat-model");

  const previousProvider = providerSelect.value;
  const previousModel = modelSelect.value;

  providerSelect.replaceChildren();
  modelSelect.replaceChildren();

  for (const provider of currentProviders) {
    const option = document.createElement("option");
    option.value = provider.id;
    option.textContent = provider.name;
    providerSelect.appendChild(option);
  }

  const active = currentProviders.find((p) => p.active) ?? currentProviders[0];
  if (!active) {
    providerSelect.disabled = true;
    modelSelect.disabled = true;
    setChip(pick<HTMLElement>("#chat-chip"), "没有 provider", "warn");
    return;
  }
  providerSelect.disabled = false;
  modelSelect.disabled = false;

  // Keep the user's selection when it is still there; otherwise fall back to
  // the active provider.
  if (currentProviders.some((p) => p.id === previousProvider)) {
    providerSelect.value = previousProvider;
  } else {
    providerSelect.value = active.id;
  }

  const chosen = currentProviders.find((p) => p.id === providerSelect.value) ?? active;
  if (chosen.defaultModel) {
    const option = document.createElement("option");
    option.value = chosen.defaultModel;
    option.textContent = chosen.defaultModel;
    modelSelect.appendChild(option);
    if (previousModel === chosen.defaultModel) {
      modelSelect.value = previousModel;
    }
  }

  setChip(
    pick<HTMLElement>("#chat-chip"),
    chosen.defaultModel ? chosen.name : "未选模型",
    chosen.defaultModel ? "ok" : "warn",
  );
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

/// Append a line to the chat log.
function appendChat(role: "user" | "assistant", text: string): HTMLElement {
  const log = pick<HTMLElement>("#chat-log");
  // The placeholder is optional: it is gone after the first message.
  log.querySelector("#chat-empty")?.remove();

  const bubble = document.createElement("div");
  bubble.className = `chat__msg chat__msg--${role}`;
  bubble.textContent = text;
  log.appendChild(bubble);
  log.scrollTop = log.scrollHeight;
  return bubble;
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

/// Send a message to the model and stream the answer back.
async function sendMessage(text: string) {
  const error = pick<HTMLElement>("#chat-error");
  error.textContent = "";

  const providerId = pick<HTMLSelectElement>("#chat-provider").value;
  const model = pick<HTMLSelectElement>("#chat-model").value;
  const send = pick<HTMLButtonElement>("#chat-send");

  appendChat("user", text);
  const bubble = appendChat("assistant", "");

  send.disabled = true;
  const channel = new Channel<ChatChunk>();
  channel.onmessage = (chunk: ChatChunk) => {
    if (chunk.error) {
      error.textContent = chunk.error;
      bubble.textContent = chunk.text || "（没有内容）";
      return;
    }
    bubble.textContent = chunk.text;
    const log = pick<HTMLElement>("#chat-log");
    log.scrollTop = log.scrollHeight;
  };

  try {
    await invoke("chat_send", {
      channel,
      message: text,
      model: model || null,
      providerId,
    });
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    error.textContent = detail;
    if (!bubble.textContent) bubble.textContent = "（没有内容）";
  } finally {
    send.disabled = false;
  }
}

async function main() {
  const refresh = async () => {
    const status = await invoke<ShellStatus>("shell_status");
    renderMemory(status);
    renderProviders(status.providers);
    renderTray(status);
  };

  // The memory layer is optional, and connecting to it is not instant: the
  // handshake can take ten seconds, or hang on a server that never
  // answers. Awaiting it here would hold the whole first screen hostage —
  // the repository list would not appear until the memory layer had
  // finished failing. So the two run side by side instead.
  void refresh().catch((error: unknown) => {
    // The frontend also runs in a plain browser (npm run dev without
    // Tauri), where there is no backend to ask. Say that instead of
    // showing an empty screen that reads like a broken install.
    const detail = error instanceof Error ? error.message : String(error);
    setChip(pick<HTMLElement>("#memory-chip"), "后端不可达", "warn");
    pick<HTMLElement>("#memory-path").textContent = detail;
  });

  // The repository screen and the announcement channel.
  //
  // The listener is registered before the first scan, so a repository
  // found while the window is opening is not missed.
  void watchForNewRepos();
  void loadRepos();

  pick<HTMLFormElement>("#root-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const input = pick<HTMLInputElement>("#root-input");
    const path = input.value.trim();
    if (!path) return;
    input.value = "";

    void invoke<RepoScan>("repo_roots_add", { path }).then(
      (scan: RepoScan) => {
        renderRepos(scan);
        queueNewRepos(scan.newRepos);
      },
      (err: unknown) => {
        pick<HTMLElement>("#repo-error").textContent = String(err);
      },
    );
  });

  pick<HTMLButtonElement>("#repo-scan").addEventListener("click", () => {
    void invoke<RepoScan>("repo_scan").then(
      (scan: RepoScan) => {
        renderRepos(scan);
        queueNewRepos(scan.newRepos);
      },
      (err: unknown) => {
        pick<HTMLElement>("#repo-error").textContent = String(err);
      },
    );
  });

  pick<HTMLButtonElement>("#toast-favorite").addEventListener("click", () => {
    answerToast(true);
  });

  pick<HTMLButtonElement>("#toast-dismiss").addEventListener("click", () => {
    answerToast(false);
  });

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
        renderProviders(list);
      },
      (err: unknown) => {
        editorError.textContent = err instanceof Error ? err.message : String(err);
      },
    );
  });

  pick<HTMLButtonElement>("#provider-fetch-models").addEventListener("click", () => {
    const values = editorValues();
    const editorError = pick<HTMLElement>("#editor-error");
    const editorDetail = pick<HTMLElement>("#editor-detail");
    editorError.textContent = "";
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

  // --- the chat ---------------------------------------------------------

  pick<HTMLSelectElement>("#chat-provider").addEventListener("change", () => {
    renderModelPickers();
  });

  pick<HTMLFormElement>("#chat-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const input = pick<HTMLTextAreaElement>("#chat-input");
    const text = input.value.trim();
    if (!text) return;
    input.value = "";
    void sendMessage(text);
  });
}

main();
