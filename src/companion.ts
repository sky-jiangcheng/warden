import { listen } from "@tauri-apps/api/event";
import { desktop, invoke, pick } from "./ui";

/// The desk pet is a resident surface, not a miniature app window. It
/// breathes while idle, reacts to hover / click / drag, and shows a thinking
/// state while the model is answering. Clicking it opens the chat overlay.
///
/// Nothing else belongs here: numbers live in the chat (the model reads them
/// through the memory layer's tools), and every entry point — click, ⌥Space,
/// the tray menu — already opens the same window, so a button that repeats
/// one of them is just chrome on a character.
///
/// The mood lives on `data-mood` so CSS owns every animation; this file only
/// decides WHEN. No timers for idle motion (CSS loops), one timer for the
/// brief reaction bubbles.
type Mood = "idle" | "curious" | "happy" | "dragging" | "thinking" | "error";

const REACTIONS: Record<Exclude<Mood, "idle" | "thinking">, string> = {
  curious: "想聊点什么？",
  happy: "这就来。",
  dragging: "放这儿。",
  error: "打不开，再点一次。",
};

export function initCompanion(): void {
  const surface = pick<HTMLElement>("#companion-surface");
  const bubble = pick<HTMLElement>("#companion-bubble");
  const mark = pick<HTMLElement>("#companion-drag");
  let opening = false;
  let dragging = false;
  let bubbleTimer: ReturnType<typeof setTimeout> | undefined;
  let moodTimer: ReturnType<typeof setTimeout> | undefined;

  function setMood(mood: Mood, ms = 0): void {
    surface.dataset.mood = mood;
    if (moodTimer) clearTimeout(moodTimer);
    if (ms > 0) {
      moodTimer = setTimeout(() => {
        if (!dragging) surface.dataset.mood = "idle";
      }, ms);
    }
  }

  function say(text: string, ms = 2600): void {
    bubble.textContent = text;
    bubble.dataset.visible = "true";
    if (bubbleTimer) clearTimeout(bubbleTimer);
    bubbleTimer = setTimeout(() => { bubble.dataset.visible = "false"; }, ms);
  }

  function fail(): void {
    setMood("error", 1600);
    say(REACTIONS.error, 3200);
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
    } catch {
      fail();
    } finally { opening = false; }
  }

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
    void invoke("window_drag").catch(() => { fail(); }).finally(() => {
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
}
