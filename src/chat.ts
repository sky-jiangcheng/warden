import { Channel } from "@tauri-apps/api/core";
import { browserNote, desktop, errorText, invoke, openSheet, pick, poll } from "./ui";
import { COMPOSITION_GRACE_MS, draftAfterSend, readIsStale, sendBlocked, shouldAdoptThread } from "./chat-state";

type Thread = { id: string; title: string; path: string | null; createdAt: string; updatedAt: string };
type Message = { id: string; role: "user" | "assistant"; content: string; source: string; replyTo: string | null; createdAt: string; pending?: boolean; failed?: boolean };
// A tool call belongs to one topic's reply; different threads can generate at
// the same time, so this state cannot live in a shared closure.
type ToolState = { name: string; done: boolean; ok?: boolean; error?: string; command?: string };
type Topic = {
  key: string; thread: Thread | null; title: string; path: string | null;
  messages: Message[]; draft: string; busy: boolean; error: string; loaded: boolean;
  revision: number; scrollTop: number; pinned: boolean; readError: boolean;
  tool: ToolState | null;
};
type Provider = { id: string; name: string; defaultModel: string | null; active: boolean };
type Repo = { path: string; name: string; missing: boolean };
type Model = { id: string; name: string | null };
type ChatChunk = { delta: string; text: string; done: boolean; error: string | null; toolCall?: { name: string; done: boolean; ok?: boolean; error?: string; command?: string } | null };

export function initChat() {
  const input = pick<HTMLTextAreaElement>("#chat-input");
  const send = pick<HTMLButtonElement>("#chat-send");
  const log = pick<HTMLElement>("#chat-log");
  const messages = pick<HTMLElement>("#chat-messages");
  const providerSelect = pick<HTMLSelectElement>("#chat-provider");
  const modelSelect = pick<HTMLSelectElement>("#chat-model");
  const contextSelect = pick<HTMLSelectElement>("#chat-context");
  const topics = new Map<string, Topic>();
  let providers: Provider[] = [];
  let repos: Repo[] = [];
  let active = makeTopic();
  let syncOnline = false;
  let syncChecked = !desktop;
  let syncProblem = "";
  let listSignature = "";
  let renderedKey = "";
  let modelGeneration = 0;
  let composing = false;
  let compositionEnded = 0;
  let renameBusy = false;
  // The tool state lives on Topic (see above), not here: concurrent replies on
  // different threads must not overwrite one shared variable.
  const modelCache = new Map<string, Model[]>();

  function makeTopic(thread?: Thread): Topic {
    const topic: Topic = {
      key: thread?.id ?? crypto.randomUUID(), thread: thread ?? null,
      title: thread?.title ?? "新对话", path: thread?.path ?? null,
      messages: [], draft: "", busy: false, error: "", loaded: !thread,
      revision: 0, scrollTop: 0, pinned: true, readError: false, tool: null,
    };
    topics.set(topic.key, topic);
    return topic;
  }

  function temporary(topic: Topic): boolean {
    return !topic.thread && (topic.messages.length > 0 || !syncOnline);
  }

  function updateStatus(): void {
    const brand = pick<HTMLElement>("#brand-chip");
    brand.textContent = !desktop ? "浏览器预览 · 未连接" : syncOnline ? "Cinder 已连接" : syncChecked ? "本机运行 · 未同步" : "本机运行 · 连接中";
    brand.dataset.connected = String(desktop && syncOnline);
    const status = pick<HTMLElement>("#sync-status");
    status.textContent = active.busy ? "正在回复" : active.readError ? "读取失败 · 未同步" : active.thread ? (syncOnline ? "已同步" : "未同步") : temporary(active) ? "临时对话 / 未同步" : "新主题";
    status.title = active.thread ? "主题通过 Cinder 共享；可见时每 2.5 秒检查更新。" : "临时对话只在本窗口保留；不向模型发送历史，不会伪装成已保存。";
    pick<HTMLButtonElement>("#sync-retry").hidden = !desktop || (!active.readError && syncOnline);
    pick<HTMLElement>("#topic-title").textContent = active.title;
    pick<HTMLElement>("#chat-error").textContent = active.error;
    const canSend = desktop && !active.busy && active.loaded && !active.readError && Boolean(input.value.trim()) && Boolean(providerSelect.value) && Boolean(modelSelect.value);
    send.disabled = !canSend;
    send.setAttribute("aria-label", active.busy ? "正在回复，请稍候" : "发送消息");
    pick<HTMLElement>("#chat-form").setAttribute("aria-busy", String(active.busy));
    contextSelect.disabled = Boolean(active.thread) || active.messages.length > 0 || active.busy;
    pick<HTMLElement>("#chat-chip").textContent = !desktop ? "预览模式" : active.busy ? "正在思考…" : !providerSelect.value ? "在设置中连接模型" : !modelSelect.value ? "请选择模型" : active.thread ? "由 Cinder 保存对话" : temporary(active) ? "临时单轮 · 退出 Warden 后不保留" : "发送后创建共享主题";
  }

  function render(): void {
    const switched = renderedKey !== active.key;
    if (switched) {
      messages.replaceChildren();
      renderedKey = active.key;
    }
    const keepBottom = active.pinned;
    const oldTop = switched ? active.scrollTop : log.scrollTop;
    const nodes = new Map(Array.from(messages.children).map((node) => [(node as HTMLElement).dataset.messageId, node as HTMLElement]));
    const ids = new Set(active.messages.map((message) => message.id));
    for (const [id, node] of nodes) if (!id || !ids.has(id)) node.remove();
    for (const message of active.messages) {
      let node = nodes.get(message.id);
      if (!node) {
        node = document.createElement("article");
        node.dataset.messageId = message.id;
        node.className = `chat__msg chat__msg--${message.role}`;
        const meta = document.createElement("span");
        meta.className = "chat__msg__meta";
        const text = document.createElement("div");
        text.className = "chat__msg__text";
        node.append(meta, text);
        messages.append(node);
      }
      node.dataset.pending = String(Boolean(message.pending));
      node.dataset.failed = String(Boolean(message.failed));
      // A shared thread also carries replies written from an IDE plugin;
      // calling those "Warden" would claim another client's answer.
      const label = message.role === "user" ? "你" : message.source === "warden" ? "Warden" : "助手";
      const source = message.source && message.source !== "warden" ? ` · ${message.source}` : "";
      // Only the latest assistant reply carries this topic's tool state.
      const lastMessage = active.messages[active.messages.length - 1] === message;
      const tool = active.tool;
      let tooling = "";
      if (tool && message.role === "assistant" && lastMessage) {
        // Phase 3: 所有命令必须显示完整命令. A local execution reports the
        // argv it really ran, so the chip reads `git --no-pager status …`
        // instead of just a tool name. MCP tools carry no local command and
        // keep the name-only form.
        const what = tool.command ? `${tool.name}（${tool.command}）` : tool.name;
        tooling = !tool.done ? ` · 调用 ${what}…`
          : tool.ok === false ? ` · ${tool.name} 失败：${tool.error ?? "未知错误"}`
          : ` · ✓ ${what} 已完成`;
      }
      const meta = `${label}${source}${tooling || (message.failed ? " · 未完成" : "")}`;
      if (node.firstElementChild!.textContent !== meta) node.firstElementChild!.textContent = meta;
      // Plain text is deliberate: model output, titles, paths and sources are
      // untrusted. Never interpolate them into markup, selectors, or URLs.
      if (node.lastElementChild!.textContent !== message.content) node.lastElementChild!.textContent = message.content;
    }
    pick<HTMLElement>("#chat-loading").hidden = active.loaded || !active.thread;
    pick<HTMLElement>("#chat-empty").hidden = active.messages.length > 0 || !active.loaded;
    log.scrollTop = keepBottom ? log.scrollHeight : oldTop;
    pick<HTMLButtonElement>("#jump-latest").hidden = keepBottom || active.messages.length === 0;
    updateStatus();
  }

  function renderTopics(): void {
    const ordered = [...topics.values()].sort((a, b) => (b.thread?.updatedAt ?? "9999").localeCompare(a.thread?.updatedAt ?? "9999"));
    const signature = JSON.stringify(ordered.map((topic) => [topic.key, topic.title, topic.thread?.updatedAt, topic.busy, Boolean(topic.draft), topic === active, temporary(topic)]));
    if (signature === listSignature) return;
    listSignature = signature;
    const list = pick<HTMLElement>("#topic-list");
    const focusedKey = (document.activeElement as HTMLElement | null)?.closest<HTMLButtonElement>("[data-topic]")?.dataset.topic;
    list.replaceChildren();
    for (const topic of ordered) {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "topic-item";
      button.dataset.topic = topic.key;
      button.setAttribute("aria-current", String(topic === active));
      const title = document.createElement("strong");
      title.textContent = topic.title;
      const detail = document.createElement("small");
      detail.textContent = [topic.thread ? "Cinder 共享" : "临时对话 / 未同步", topic.path?.split(/[\\/]/).pop(), topic.busy ? "回复中" : topic.draft ? "有草稿" : ""].filter(Boolean).join(" · ");
      button.append(title, detail);
      button.addEventListener("click", () => selectTopic(topic));
      item.append(button);
      list.append(item);
      if (focusedKey === topic.key) button.focus();
    }
  }

  function syncContext(): void {
    contextSelect.replaceChildren(new Option("不附加仓库", ""));
    for (const repo of repos) {
      if (!repo.missing || repo.path === active.path) contextSelect.add(new Option(repo.name, repo.path));
    }
    if (active.path && !repos.some((repo) => repo.path === active.path)) contextSelect.add(new Option(active.path.split(/[\\/]/).pop() || active.path, active.path));
    contextSelect.value = active.path ?? "";
    contextSelect.title = active.path ?? "对话仓库，发送后固定";
  }

  function selectTopic(topic: Topic): void {
    active.draft = input.value;
    active.scrollTop = log.scrollTop;
    active = topic;
    input.value = active.draft;
    resizeInput();
    syncContext();
    pick<HTMLInputElement>("#topic-rename").value = active.title;
    pick<HTMLElement>("#topic-error").textContent = "";
    pick<HTMLDialogElement>("#topics-sheet").close();
    render();
    renderTopics();
    readPoll.refresh();
    input.focus({ preventScroll: true });
  }

  function showTopics(): void {
    renderTopics();
    pick<HTMLInputElement>("#topic-rename").value = active.title;
    pick<HTMLElement>("#topic-error").textContent = "";
    openSheet("#topics-sheet");
  }

  async function fetchThreads(): Promise<void> {
    // Snapshot each revision before the request: a local rename bumps it, and
    // an in-flight rename must not be overwritten by an old server title.
    const revisions = new Map([...topics.values()].map((topic) => [topic.key, topic.revision]));
    try {
      const result = await invoke<{ threads: Thread[] }>("conversation_list", { path: null });
      for (const thread of result.threads) {
        const topic = [...topics.values()].find((item) => item.thread?.id === thread.id);
        if (topic) {
          // A local rename/create/send can finish while this list is in flight.
          if (shouldAdoptThread({
            revisionUnchanged: revisions.get(topic.key) === topic.revision,
            busy: topic.busy,
            hasLocalThread: Boolean(topic.thread),
            serverUpdatedAt: thread.updatedAt,
            localUpdatedAt: topic.thread?.updatedAt ?? "",
          })) {
            topic.thread = thread;
            topic.title = thread.title;
          }
        } else makeTopic(thread);
      }
      syncOnline = true;
      syncProblem = "";
    } catch (error) {
      syncOnline = false;
      syncProblem = errorText(error);
      throw error;
    } finally {
      syncChecked = true;
      pick<HTMLElement>("#topics-note").textContent = syncProblem ? `同步暂不可用，将自动重试。${syncProblem}` : "共享主题由 Cinder 同步；临时对话只保留在当前窗口。";
      renderTopics();
      updateStatus();
    }
  }

  async function readActive(): Promise<void> {
    const topic = active;
    if (!topic.thread || topic.busy) return;
    const revision = topic.revision;
    try {
      const result = await invoke<{ thread: Thread; messages: Message[] }>("conversation_read", { threadId: topic.thread.id });
      if (readIsStale({ revisionAtRequest: revision, revisionNow: topic.revision, busy: topic.busy })) return;
      topic.thread = result.thread;
      topic.title = result.thread.title;
      topic.path = result.thread.path;
      topic.messages = result.messages;
      topic.loaded = true;
      if (topic.readError) topic.error = "";
      topic.readError = false;
      if (active === topic) { syncContext(); render(); }
      renderTopics();
    } catch (error) {
      if (readIsStale({ revisionAtRequest: revision, revisionNow: topic.revision, busy: topic.busy })) return;
      topic.readError = true;
      topic.error = `主题暂时无法读取，草稿已保留，将自动重试。${errorText(error)}`;
      if (active === topic) updateStatus();
      throw error;
    }
  }

  function fillModels(models: Model[], preferred: string): void {
    modelSelect.replaceChildren(new Option("选择模型", ""));
    for (const model of models) modelSelect.add(new Option(model.name || model.id, model.id));
    if (models.some((model) => model.id === preferred)) modelSelect.value = preferred;
    modelSelect.disabled = !desktop || models.length === 0;
    updateStatus();
  }

  async function loadModels(preferred = ""): Promise<void> {
    const generation = ++modelGeneration;
    const provider = providers.find((item) => item.id === providerSelect.value);
    pick<HTMLElement>("#model-error").textContent = "";
    if (!provider) { fillModels([], ""); return; }
    const cached = modelCache.get(provider.id);
    const defaults: Model[] = provider.defaultModel ? [{ id: provider.defaultModel, name: null }] : [];
    fillModels(cached ?? defaults, preferred || provider.defaultModel || "");
    if (cached) return;
    try {
      const models = await invoke<Model[]>("provider_fetch_models", { id: provider.id, baseUrl: null, apiKey: null });
      modelCache.set(provider.id, models);
      if (generation !== modelGeneration) return;
      const choices = [...models];
      if (provider.defaultModel && !models.some((model) => model.id === provider.defaultModel)) choices.unshift(defaults[0]);
      fillModels(choices, modelSelect.value || preferred || provider.defaultModel || "");
    } catch (error) {
      if (generation === modelGeneration) pick<HTMLElement>("#model-error").textContent = `模型列表暂不可用${provider.defaultModel ? "，仍可使用默认模型" : "，请在设置中重试"}。${errorText(error)}`;
    }
  }

  function setProviders(next: Provider[]): void {
    const previousProvider = providerSelect.value;
    const previousModel = modelSelect.value;
    providers = next;
    modelCache.clear();
    providerSelect.replaceChildren();
    for (const provider of providers) providerSelect.add(new Option(provider.name, provider.id));
    if (!providers.length) providerSelect.add(new Option("连接 provider", ""));
    providerSelect.value = providers.some((provider) => provider.id === previousProvider) ? previousProvider : (providers.find((provider) => provider.active) ?? providers[0])?.id ?? "";
    providerSelect.disabled = !desktop || !providers.length;
    void loadModels(previousProvider === providerSelect.value ? previousModel : "");
    updateStatus();
  }

  async function sendMessage(): Promise<void> {
    const topic = active;
    const draft = input.value;
    const text = draft.trim();
    if (sendBlocked({ composing, msSinceCompositionEnd: Date.now() - compositionEnded, busy: topic.busy, text })) return;
    if (!desktop) { topic.error = browserNote; updateStatus(); return; }
    if (!topic.loaded || topic.readError) { topic.error = "请等待主题读取成功后再发送，草稿会保留。"; updateStatus(); return; }
    const providerId = providerSelect.value;
    const model = modelSelect.value;
    if (!providerId || !model) { topic.error = "请先在设置中连接 provider，并选择模型。"; updateStatus(); return; }
    topic.busy = true; // Set before the first await; Enter and submit cannot double-send.
    topic.error = "";
    topic.draft = draft;
    topic.revision += 1;
    topic.tool = null;
    updateStatus();
    renderTopics();
    const messageId = crypto.randomUUID();
    let assistant: Message | undefined;
    let user: Message | undefined;
    let accepting = true;
    let streamError = "";
    let streamDone = false;
    let sendFailed = false;
    try {
      // Never silently migrate a temporary exchange into a persistent thread:
      // it did not have model history or shared storage. Only an empty topic
      // can be created in Cinder once the connection is known to work.
      if (!topic.thread && topic.messages.length === 0 && syncOnline) {
        const title = topic.title === "新对话" ? text.slice(0, 36) : topic.title;
        const result = await invoke<{ thread: Thread }>("conversation_create", { id: topic.key, title, path: topic.path });
        topic.thread = result.thread;
        topic.title = result.thread.title;
      } else if (topic.title === "新对话") topic.title = text.slice(0, 36);
      const createdAt = new Date().toISOString();
      user = { id: messageId, role: "user", content: text, source: "warden", replyTo: null, createdAt, pending: true };
      assistant = { id: crypto.randomUUID(), role: "assistant", content: "", source: "warden", replyTo: messageId, createdAt, pending: true };
      topic.messages.push(user, assistant);
      if (active === topic) render();
      const answer = assistant;
      const channel = new Channel<ChatChunk>();
      channel.onmessage = (chunk) => {
        if (!accepting || streamDone) return;
        if (typeof chunk.text === "string") answer.content = chunk.text;
        else answer.content += chunk.delta || "";
        if (chunk.toolCall) {
          topic.tool = {
            name: chunk.toolCall.name,
            done: chunk.toolCall.done,
            ok: chunk.toolCall.ok,
            error: chunk.toolCall.error,
            command: chunk.toolCall.command,
          };
        }
        if (chunk.error) streamError = chunk.error;
        if (chunk.done || chunk.error) streamDone = true;
        // Captured topic, not whichever topic is visible when a late chunk arrives.
        if (active === topic) render();
      };
      await invoke("chat_send", { channel, message: text, model, providerId, threadId: topic.thread?.id ?? null, messageId });
      if (streamError) throw new Error(streamError);
      if (!answer.content) throw new Error("未收到模型回复，请重试。");
      // The Rust command returns after appending the assistant to Cinder.
      // A user may already be editing the next draft: never erase that work.
      // The draft itself is settled once in `finally` (draftAfterSend), so the
      // keep-on-failure rule and the keep-on-edit rule share one code path.
      if (active === topic && input.value === draft) { input.value = ""; resizeInput(); }
      pick<HTMLElement>("#chat-announcement").textContent = active === topic ? "Warden 已回复。" : `主题「${topic.title}」已收到回复。`;
    } catch (error) {
      sendFailed = true;
      topic.error = `发送未完成，草稿已保留。${errorText(error)}`;
      if (user) user.failed = true;
      if (assistant) assistant.failed = true;
      // In particular, a create failure must not secretly fall back to an
      // unsynced send; the user gets to retry the intended operation.
    } finally {
      accepting = false;
      // Keep the last tool state so done / failed stays visible after the
      // stream ends, and surface a tool failure in the alert region.
      // The onmessage callback assigns it asynchronously, so read it back
      // through the map instead of a control-flow-narrowed local field.
      const finalTool = topics.get(topic.key)?.tool ?? null;
      if (finalTool?.done && finalTool.ok === false && !topic.error) {
        topic.error = `工具调用失败：${finalTool.name}${finalTool.error ? ` · ${finalTool.error}` : ""}。`;
      }
      if (user) user.pending = false;
      if (assistant) assistant.pending = false;
      // One settle point for the draft rule (chat-state): failure keeps the
      // draft, success with a re-edited draft keeps the new work, success
      // untouched clears it. On success topic.draft was already reset above
      // when the input still held the sent text.
      topic.draft = draftAfterSend({ draftNow: topic.draft, draftAtSend: draft, failed: sendFailed });
      topic.busy = false;
      topic.revision += 1;
      if (active === topic) render();
      renderTopics();
      listPoll.refresh();
      // On error keep the partial reply visible until the next normal poll.
      if (!topic.error) readPoll.refresh();
    }
  }

  function resizeInput(): void {
    input.style.height = "auto";
    // Read the bounds from CSS instead of repeating 65/132 here: the short
    // viewport breakpoint lowers min-height, and an inline pixel floor was
    // overriding it (the composer then grew past the height it was given).
    const styles = getComputedStyle(input);
    const min = Number.parseFloat(styles.minHeight);
    const max = Number.parseFloat(styles.maxHeight);
    const floor = Number.isFinite(min) ? min : 65;
    const ceiling = Number.isFinite(max) ? max : 132;
    input.style.height = `${Math.min(Math.max(input.scrollHeight, floor), ceiling)}px`;
  }

  input.addEventListener("compositionstart", () => { composing = true; });
  input.addEventListener("compositionend", () => { composing = false; compositionEnded = Date.now(); });
  input.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.shiftKey) return;
    if (event.isComposing || composing || event.keyCode === 229 || Date.now() - compositionEnded < COMPOSITION_GRACE_MS) return;
    event.preventDefault();
    void sendMessage();
  });
  input.addEventListener("input", () => { active.draft = input.value; resizeInput(); updateStatus(); });
  pick<HTMLFormElement>("#chat-form").addEventListener("submit", (event) => { event.preventDefault(); void sendMessage(); });
  providerSelect.addEventListener("change", () => { void loadModels(); });
  modelSelect.addEventListener("change", updateStatus);
  contextSelect.addEventListener("change", () => {
    active.path = contextSelect.value || null;
    contextSelect.title = active.path ?? "对话仓库，发送后固定";
  });
  log.addEventListener("scroll", () => {
    active.pinned = log.scrollHeight - log.scrollTop - log.clientHeight < 48;
    active.scrollTop = log.scrollTop;
    pick<HTMLButtonElement>("#jump-latest").hidden = active.pinned || active.messages.length === 0;
  }, { passive: true });
  pick<HTMLButtonElement>("#jump-latest").addEventListener("click", () => { active.pinned = true; render(); });
  for (const button of document.querySelectorAll<HTMLButtonElement>("[data-suggestion]")) {
    button.addEventListener("click", () => {
      input.value = button.dataset.suggestion ?? "";
      active.draft = input.value;
      resizeInput();
      updateStatus();
      input.focus();
    });
  }
  pick<HTMLButtonElement>("#topics-open").addEventListener("click", showTopics);
  pick<HTMLButtonElement>("#topic-title").addEventListener("click", showTopics);
  for (const id of ["#topic-new", "#topic-new-in-sheet"]) pick<HTMLButtonElement>(id).addEventListener("click", () => selectTopic(makeTopic()));
  // Settings are a page owned by main.ts; this button only exists as the
  // quiet in-window entry point, so no dialog is opened here.
  pick<HTMLButtonElement>("#chat-hide").addEventListener("click", () => {
    if (!desktop) { active.error = "浏览器预览无法收起桌面窗口。"; updateStatus(); return; }
    void invoke("companion_hide_chat").catch((error) => { active.error = `暂时无法收起。${errorText(error)}`; updateStatus(); });
  });
  // The window is borderless, so its header is the drag handle. Controls
  // inside the header keep their own clicks: only empty space drags. The
  // settings tab strip belongs to that same band, so it drags too — and the
  // button guard means clicking a tab never does.
  for (const id of ["#main-drag", "#settings-tabs", "#settings-drag"]) {
    pick<HTMLElement>(id).addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || !desktop) return;
      if ((event.target as HTMLElement).closest("button, input, select, textarea, a, label")) return;
      void invoke("window_drag").catch((error) => {
        active.error = `暂时无法拖动窗口。${errorText(error)}`;
        updateStatus();
      });
    });
  }
  pick<HTMLFormElement>("#topic-rename-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const title = pick<HTMLInputElement>("#topic-rename").value.trim();
    if (!title || renameBusy || active.busy) return;
    const topic = active;
    const button = pick<HTMLButtonElement>("#topic-rename-save");
    renameBusy = true;
    button.disabled = true;
    topic.revision += 1;
    void (async () => {
      try {
        if (topic.thread) {
          const result = await invoke<{ thread: Thread }>("conversation_rename", { threadId: topic.thread.id, title });
          topic.thread = result.thread;
          topic.title = result.thread.title;
        } else topic.title = title;
        topic.revision += 1;
        renderTopics();
        updateStatus();
        pick<HTMLElement>("#topic-error").textContent = "";
      } catch (error) { pick<HTMLElement>("#topic-error").textContent = errorText(error); }
      finally { renameBusy = false; button.disabled = false; }
    })();
  });

  // The preview-only note stays hidden on desktop; the memory banner is
  // driven separately from shell_status and is visible on desktop too.
  pick<HTMLElement>("#environment-note").hidden = desktop;
  providerSelect.disabled = true;
  modelSelect.disabled = true;
  render();
  const listPoll = poll(fetchThreads, 2500);
  const readPoll = poll(readActive, 2500);
  pick<HTMLButtonElement>("#sync-retry").addEventListener("click", () => { listPoll.refresh(); readPoll.refresh(); });
  window.addEventListener("pagehide", () => { listPoll.stop(); readPoll.stop(); }, { once: true });

  return {
    setProviders,
    setRepos(next: Repo[]) { repos = next; syncContext(); },
    retrySync() { listPoll.refresh(); readPoll.refresh(); },
  };
}
