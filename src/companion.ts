import { count, desktop, errorText, invoke, pick, poll, type WorkspaceMetrics } from "./ui";

export function initCompanion(): void {
  const status = pick<HTMLElement>("#companion-status");
  const metric = pick<HTMLElement>("#companion-metric");
  let opening = false;
  let dragging = false;
  if (!desktop) {
    status.textContent = "浏览器预览 · 未连接";
    metric.textContent = "工作一览 · 未连接";
  }
  async function showChat(): Promise<void> {
    if (opening) return;
    if (!desktop) {
      // A real navigable preview, not a simulated desktop command.
      const target = new URL(location.href);
      target.searchParams.delete("surface");
      location.assign(target.href);
      return;
    }
    opening = true;
    try {
      await invoke("companion_show_chat");
      status.textContent = "";
    } catch (error) {
      status.textContent = "打开失败，点击重试";
      status.title = errorText(error);
    } finally { opening = false; }
  }
  pick<HTMLButtonElement>("#companion-open").addEventListener("click", () => { void showChat(); });
  // No data-tauri-drag-region on controls: only the lantern body initiates a
  // native window drag. The illustration itself is not a fake voice button.
  pick<HTMLElement>("#companion-drag").addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || !desktop || dragging) return;
    event.preventDefault();
    dragging = true;
    void invoke("window_drag").catch((error) => {
      status.textContent = "暂时无法移动";
      status.title = errorText(error);
    }).finally(() => { dragging = false; });
  });
  const metricsPoll = poll(async () => {
    try {
      const result = await invoke<WorkspaceMetrics>("workspace_metrics", { path: null });
      metric.textContent = `${count(result.totalLines)} 行 · ${count(result.commitsToday)} 次提交`;
      metric.title = "工作一览";
      if (status.textContent === "指标未同步 · 自动重试") status.textContent = "";
    } catch (error) {
      metric.title = errorText(error);
      status.textContent = "指标未同步 · 自动重试";
      throw error;
    }
  }, 15000);
  window.addEventListener("pagehide", () => metricsPoll.stop(), { once: true });
}
