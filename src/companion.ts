import { listen } from "@tauri-apps/api/event";
import { count, desktop, errorText, invoke, pick, poll, type WorkspaceMetrics } from "./ui";

/// The desk pet is a resident surface, not a miniature chat window. It
/// breathes while idle, reacts to hover / click / drag, and shows a thinking
/// state while the model is answering. Clicking it opens the chat overlay.
///
/// The mood lives on `data-mood` so CSS owns every animation; this file only
/// decides WHEN. No timers for idle motion (CSS loops), one timer for the
/// brief reaction bubbles.
type Mood = "idle" | "curious" | "happy" | "dragging" | "thinking" | "error";

const REACTIONS: Record<Exclude<Mood, "idle" | "thinking">, string> = {
  curious: "有什么想一起想一想？",
  happy: "我在，点我就能聊。",
  dragging: "放这儿就好。",
  error: "稍等，我再试试。",
};

export function initCompanion(): void {
  const surface = pick<HTMLElement>("#companion-surface");
  const status = pick<HTMLElement>("#companion-status");
  const metric = pick<HTMLElement>("#companion-metric");
  const bubble = pick<HTMLElement>("#companion-bubble");
  const mark = pick<HTMLElement>("#companion-drag");
  let opening = false;
  let dragging = false;
  let bubbleTimer: ReturnType<typeof setTimeout> | undefined;
  let moodTimer: ReturnType<typeof setTimeout> | undefined;

  if (!desktop) {
    status.textContent = "浏览器预览 · 未连接";
    metric.textContent = "工作一览 · 未连接";
  }

  function setMood(mood: Mood, ms = 0): void {
    surface.dataset.mood = mood;
    if (moodTimer) clearTimeout(moodTimer);
    if (ms > 0) {
      moodTimer = setTimeout(() => {
        if (!dragging) surface.dataset.mood = "idle";
      }, ms);
    }
  }

  function say(text: string): void {
    bubble.textContent = text;
    bubble.dataset.visible = "true";
    if (bubbleTimer) clearTimeout(bubbleTimer);
    bubbleTimer = setTimeout(() => { bubble.dataset.visible = "false"; }, 2600);
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
    setMood("happy", 900);
    say(REACTIONS.happy);
    try {
      await invoke("companion_show_chat");
      status.textContent = "";
    } catch (error) {
      setMood("error", 1600);
      status.textContent = "打开失败，点击重试";
      status.title = errorText(error);
    } finally { opening = false; }
  }

  pick<HTMLButtonElement>("#companion-open").addEventListener("click", () => { void showChat(); });
  // The mark itself is the conversation entry. Dragging starts from the same
  // element, so a drag must not be mistaken for a click.
  mark.addEventListener("click", () => { if (!dragging) void showChat(); });
  mark.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      void showChat();
    }
  });
  mark.addEventListener("pointerenter", () => {
    if (!dragging) { setMood("curious"); say(REACTIONS.curious); }
  });
  mark.addEventListener("pointerleave", () => {
    if (!dragging) setMood("idle");
  });

  mark.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || !desktop || dragging) return;
    event.preventDefault();
    dragging = true;
    setMood("dragging");
    say(REACTIONS.dragging);
    void invoke("window_drag").catch((error) => {
      setMood("error", 1600);
      status.textContent = "暂时无法移动";
      status.title = errorText(error);
    }).finally(() => {
      dragging = false;
      setMood("idle");
    });
  });

  // The chat window publishes its generation state; the pet shows it even
  // when the chat overlay is hidden (e.g. a long answer running while the
  // user clicked away).
  void listen<boolean>("warden:thinking", (event) => {
    if (event.payload) setMood("thinking");
    else setMood("idle");
  }).catch(() => { /* plain browser: no backend events */ });

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
