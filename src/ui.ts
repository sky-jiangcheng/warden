import { invoke as tauriInvoke } from "@tauri-apps/api/core";

// Checking the bridge also supports Tauri's injected browser-test mocks. No
// fake providers, messages, metrics, or connection state live in production.
export const desktop = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export const browserNote = "浏览器预览未连接本机服务，请在 Warden 桌面端使用。";

export function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!desktop) return Promise.reject(new Error(browserNote));
  return tauriInvoke<T>(command, args);
}

export function pick<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`missing element: ${selector}`);
  return element;
}

// Rust and the OS report errors in English while the UI speaks Chinese.
// Translate the common phrases; backend messages are never rewritten here.
const ERROR_PHRASES: ReadonlyArray<readonly [RegExp, string]> = [
  [/connection refused/gi, "连接被拒绝"],
  [/connection reset/gi, "连接被重置"],
  [/timed?\s?out|timeout/gi, "请求超时"],
  [/permission denied|access denied/gi, "没有权限"],
  [/no such file or directory|not found/gi, "未找到"],
  [/broken pipe/gi, "连接中断"],
  [/network (?:is )?unreachable|name resolution/gi, "网络不可达"],
  [/unauthorized|forbidden/gi, "未授权"],
  [/invalid|malformed/gi, "格式无效"],
  [/failed to|unable to/gi, "操作失败"],
];

export function errorText(error: unknown): string {
  if (!desktop) return browserNote;
  const raw = error instanceof Error ? error.message : String(error);
  if (!/[A-Za-z]/.test(raw)) return raw;
  let text = raw;
  for (const [pattern, zh] of ERROR_PHRASES) text = text.replace(pattern, zh);
  return text;
}

export function initSheets(): void {
  for (const button of document.querySelectorAll<HTMLButtonElement>("[data-close]")) {
    button.addEventListener("click", () => pick<HTMLDialogElement>(`#${button.dataset.close}`).close());
  }
  for (const dialog of document.querySelectorAll<HTMLDialogElement>("dialog")) {
    let opener: HTMLElement | null = null;
    dialog.addEventListener("close", () => {
      if (opener?.isConnected) opener.focus();
    });
    dialog.addEventListener("click", (event) => {
      const bounds = dialog.getBoundingClientRect();
      if (event.target === dialog && (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom)) dialog.close();
    });
    // Save focus before native showModal moves it into the dialog.
    dialog.addEventListener("warden:open", () => {
      opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      if (!dialog.open) dialog.showModal();
    });
  }
}

export function openSheet(id: string): void {
  pick<HTMLDialogElement>(id).dispatchEvent(new Event("warden:open"));
}

// Recursive scheduling: at most one request per poller is in flight. A
// visibility change or manual retry queues a follow-up, never overlaps it.
// The backend owns IPC timeouts; racing an uncancellable invoke here would
// leave requests running and violate the no-overlap guarantee.
export function poll(job: () => Promise<void>, interval: number): { refresh: () => void; stop: () => void } {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let running = false;
  let stopped = false;
  let requested = false;
  let failures = 0;
  const run = async (): Promise<void> => {
    if (stopped || document.hidden || !desktop) return;
    if (running) { requested = true; return; }
    running = true;
    requested = false;
    try { await job(); failures = 0; } catch { failures += 1; }
    finally {
      running = false;
      if (!stopped && !document.hidden) {
        timer = setTimeout(() => void run(), requested ? 0 : Math.min(interval * 2 ** failures, 15000));
      }
    }
  };
  const refresh = (): void => {
    clearTimeout(timer);
    void run();
  };
  const visibility = (): void => {
    clearTimeout(timer);
    if (!document.hidden) refresh();
  };
  document.addEventListener("visibilitychange", visibility);
  refresh();
  return {
    refresh,
    stop() { stopped = true; clearTimeout(timer); document.removeEventListener("visibilitychange", visibility); },
  };
}
