import { listen } from "@tauri-apps/api/event";
import { desktop, invoke, pick, poll } from "./ui";

/// The desk pet is a resident surface, not a miniature app window. It
/// breathes while idle, reacts to hover / click / drag, shows a thinking
/// state while the model is answering, and — the part that makes it a
/// character rather than an icon — keeps its eyes on the pointer wherever
/// the pointer is on the screen.
///
/// Nothing else belongs here: numbers live in the chat (the model reads them
/// through the memory layer's tools), and every entry point — click, ⌥Space,
/// the tray menu — already opens the same window, so a button that repeats
/// one of them is just chrome on a character.
///
/// The mood lives on `data-mood`, mirrored onto the surface and the pet (the
/// pet's CSS keys off its own copy), so CSS owns every animation; this file
/// only decides WHEN. The gaze lives on `data-look` and in the `--look-x` /
/// `--look-y` custom properties. Idle motion is CSS loops; the timers here
/// are the blink cycle, the sulk countdown and the reaction bubbles.
type Mood =
  | "idle"
  | "curious"
  | "happy"
  | "dragging"
  | "thinking"
  | "error"
  | "tsundere"
  | "surprised";

/// The face frames: four diagonals for the gaze, plus the resting face, the
/// blink and the half-closed sulk.
type Look =
  | "neutral"
  | "upleft"
  | "upright"
  | "downleft"
  | "downright"
  | "blink"
  | "sleepy";

const REACTIONS: Record<Exclude<Mood, "idle" | "thinking">, string> = {
  curious: "……有事？",
  happy: "哼，就一下。",
  dragging: "放、放我下来！",
  error: "才不是你的错。",
  tsundere: "不理你了。",
  surprised: "吓、吓我一跳。",
};

/// Where the pointer is relative to the pet, in the pet's own pixels.
type PointerState = {
  /// The pointer's screen position in physical pixels.
  screenX: number;
  screenY: number;
  /// The pet window's own position and size in physical pixels.
  windowX: number;
  windowY: number;
  windowWidth: number;
  windowHeight: number;
  /// Physical pixels per logical pixel.
  scaleFactor: number;
};

/// The cat notices the pointer within roughly a cat-width of its body.
/// Farther away it keeps to itself: a pointer on the other side of the
/// screen is not something a small cat can see, and pretending otherwise
/// reads as a tracking bug.
const ATTENTION_RADIUS = 104;

/// A pointer that jumps from outside the attention radius to inside this one
/// between two polls moved fast and landed close: a startle, not a glance.
const STARTLE_RADIUS = 56;

/// Below this an axis counts as "straight ahead", so a pointer sitting on
/// the cat does not make it twitch. While an axis is in the dead zone the
/// last decided side is kept, so crossing the zone does not flip the face
/// between mirror frames.
const DIRECTION_DEADZONE = 6;

/// How often the pointer is read. The OS call is cheap, but polling it on
/// every animation frame would burn a core for a 130-pixel window; 60ms is
/// smooth enough for a glance and cheap enough to be invisible.
const POINTER_POLL_MS = 60;

/// The blink cycle. Cats blink roughly every few seconds; the frame swap is
/// short enough to read as a blink rather than a change of expression.
const BLINK_INTERVAL_MS = 3400;
const BLINK_DURATION_MS = 130;

/// After this long with no interaction the cat stops paying attention and
/// sulks. It is the tsundere beat: it wants to be noticed.
const SULK_AFTER_MS = 12000;

export function initCompanion(): void {
  const surface = pick<HTMLElement>("#companion-surface");
  const bubble = pick<HTMLElement>("#companion-bubble");
  const mark = pick<HTMLElement>("#companion-drag");
  const pet = pick<HTMLElement>("#pet");

  let opening = false;
  let dragging = false;
  let thinking = false;
  let sulking = false;
  let bubbleTimer: ReturnType<typeof setTimeout> | undefined;
  let moodTimer: ReturnType<typeof setTimeout> | undefined;
  let sulkTimer: ReturnType<typeof setTimeout> | undefined;

  /// The gaze direction the cat returns to after a blink. "blink" itself is
  /// never stored here: it is a frame shown on top of the gaze, not a gaze.
  let look: Look = "neutral";
  /// Set while a blink frame is showing, so the poll does not fight it.
  let blinking = false;
  /// Whether the pointer is close enough to be worth looking at.
  let attention = false;
  let lastSide: { h: "left" | "right"; v: "up" | "down" } = { h: "left", v: "up" };
  let lastDistance = Infinity;

  function setMood(mood: Mood, ms = 0): void {
    surface.dataset.mood = mood;
    pet.dataset.mood = mood;
    if (moodTimer) clearTimeout(moodTimer);
    if (ms > 0) {
      moodTimer = setTimeout(() => {
        if (!dragging) {
          surface.dataset.mood = "idle";
          pet.dataset.mood = "idle";
        }
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

  function applyLook(next: Look): void {
    look = next;
    if (!blinking) pet.dataset.look = next;
  }

  /// Eyes back to the centre, head straight.
  function rest(): void {
    pet.style.setProperty("--look-x", "0");
    pet.style.setProperty("--look-y", "0");
    applyLook("neutral");
  }

  /// The pointer's direction, as one of the four diagonals the art covers.
  function directionFrom(x: number, y: number): Look {
    if (Math.abs(x) < DIRECTION_DEADZONE && Math.abs(y) < DIRECTION_DEADZONE) {
      return "neutral";
    }
    if (Math.abs(x) >= DIRECTION_DEADZONE) lastSide.h = x < 0 ? "left" : "right";
    if (Math.abs(y) >= DIRECTION_DEADZONE) lastSide.v = y < 0 ? "up" : "down";
    return `${lastSide.v}${lastSide.h}`;
  }

  /// The cat turns toward the pointer and its eyes follow.
  function aim(x: number, y: number): void {
    // The turn is a plain ratio, not an angle: how far the pointer is from
    // the cat, in units of the pet's own size, decides how much the head
    // turns. A pointer at the edge of the neighbourhood and one just past
    // the cat's shoulder then look different, which is what a real glance
    // does.
    const reach = 150;
    const nx = Math.max(-1, Math.min(1, x / reach));
    const ny = Math.max(-1, Math.min(1, y / reach));
    pet.style.setProperty("--look-x", nx.toFixed(3));
    pet.style.setProperty("--look-y", ny.toFixed(3));
    applyLook(directionFrom(x, y));
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
    resetSulk();
    setMood("happy", 900);
    say(REACTIONS.happy);
    try {
      await invoke("companion_show_chat");
    } catch {
      fail();
    } finally { opening = false; }
  }

  // ---- the blink cycle -------------------------------------------------
  //
  // Blinking is not decoration. A still-open stare is the single clearest
  // signal that an image is a picture, and this cat is meant to read as
  // alive; so the face blinks on its own, and the eyes keep following the
  // pointer between blinks.
  function scheduleBlink(): void {
    setTimeout(() => {
      blinking = true;
      pet.dataset.look = "blink";
      setTimeout(() => {
        blinking = false;
        pet.dataset.look = look;
        scheduleBlink();
      }, BLINK_DURATION_MS);
    }, BLINK_INTERVAL_MS + Math.random() * 1800);
  }

  // ---- the sulk --------------------------------------------------------
  //
  // Left alone long enough, the cat stops tracking the pointer and turns
  // its face away. It is the tsundere part of the character: the affection
  // has to be asked for. Any interaction — hover, click, drag, a generation
  // starting — counts as being noticed and resets the countdown.
  function resetSulk(): void {
    sulking = false;
    if (look === "sleepy") applyLook("neutral");
    if (sulkTimer) clearTimeout(sulkTimer);
    sulkTimer = setTimeout(() => {
      if (dragging || opening || thinking || document.hidden) return;
      sulking = true;
      attention = false;
      pet.style.setProperty("--look-x", "0");
      pet.style.setProperty("--look-y", "0");
      setMood("tsundere");
      applyLook("sleepy");
      say(REACTIONS.tsundere, 3200);
    }, SULK_AFTER_MS);
  }

  /// The pointer poll. It reads the OS cursor position and works out where
  /// the pointer is relative to the cat. `poll` owns the cadence: it pauses
  /// while the window is hidden and never overlaps two reads.
  async function readPointer(): Promise<void> {
    if (dragging || sulking) return;
    const state = await invoke<PointerState>("companion_pointer");
    const scale = state.scaleFactor || 1;
    // The cat sits at the bottom of the window, under the bubble: its centre
    // is the pet box's centre, not the window's.
    const rect = pet.getBoundingClientRect();
    const centreX = state.windowX + (rect.left + rect.width / 2) * scale;
    const centreY = state.windowY + (rect.top + rect.height / 2) * scale;
    const dx = (state.screenX - centreX) / scale;
    const dy = (state.screenY - centreY) / scale;
    const distance = Math.hypot(dx, dy);

    if (distance < ATTENTION_RADIUS) {
      if (!attention && lastDistance >= ATTENTION_RADIUS && distance < STARTLE_RADIUS && !thinking) {
        setMood("surprised", 650);
        say(REACTIONS.surprised, 2200);
      }
      attention = true;
      aim(dx, dy);
    } else if (attention) {
      attention = false;
      rest();
    }
    lastDistance = distance;
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
    if (!dragging) {
      setMood("curious");
      say(REACTIONS.curious);
      resetSulk();
    }
  });
  mark.addEventListener("pointerleave", () => {
    if (!dragging && !sulking) setMood("idle");
  });

  mark.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || !desktop || dragging) return;
    event.preventDefault();
    dragging = true;
    attention = false;
    lastDistance = Infinity;
    pet.style.setProperty("--look-x", "0");
    pet.style.setProperty("--look-y", "0");
    setMood("dragging");
    say(REACTIONS.dragging);
    void invoke("window_drag").catch(() => { fail(); }).finally(() => {
      dragging = false;
      setMood("idle");
      resetSulk();
    });
  });

  // The chat window publishes its generation state; the pet shows it even
  // when the chat overlay is hidden (e.g. a long answer running while the
  // user clicked away). A working cat is not an ignored cat: the sulk
  // countdown restarts on both edges.
  void listen<boolean>("warden:thinking", (event) => {
    thinking = event.payload;
    setMood(thinking ? "thinking" : "idle");
    resetSulk();
  }).catch(() => { /* plain browser: no backend events */ });

  scheduleBlink();
  resetSulk();
  if (desktop) {
    poll(readPointer, POINTER_POLL_MS);
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) {
        lastDistance = Infinity;
        resetSulk();
      }
    });
  }
}
