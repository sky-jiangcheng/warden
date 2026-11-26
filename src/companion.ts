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
/// are the blink cycle, the nap countdown and the reaction bubbles.
type Mood =
  | "idle"
  | "curious"
  | "happy"
  | "dragging"
  | "thinking"
  | "error"
  | "sleeping"
  | "surprised";

/// The gaze states. The bot's eyes are vector shapes, so "upleft" and friends
/// are no longer separate pictures — they are the same two ellipses nudged by
/// --look-x / --look-y, plus the two states CSS cannot derive from a pointer
/// position: a blink and the closed-eye nap.
type Look =
  | "neutral"
  | "upleft"
  | "upright"
  | "downleft"
  | "downright"
  | "blink"
  | "sleepy";

/// What the pet says when something happens. Sleeping is deliberately absent:
/// a napping pet has nothing to say, and a bubble would break the pose.
const REACTIONS: Record<Exclude<Mood, "idle" | "thinking" | "sleeping">, string> = {
  curious: "……有事？",
  happy: "哼，就一下。",
  dragging: "放、放我下来！",
  error: "才不是你的错。",
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

/// A pointer that jumps from far away to inside this radius between two
/// polls moved fast and landed close: a startle, not a glance.
const STARTLE_RADIUS = 56;
const STARTLE_FROM = 150;

/// The gaze follows the pointer anywhere on screen. Within GAZE_REACH the
/// turn saturates; beyond it the head turn fades with distance — the character does
/// not crane its neck across the room — while the eyes still aim.
const GAZE_REACH = 160;
const GAZE_FALLOFF = 520;

/// Exponential smoothing time constant for the gaze. The poll only sets a
/// target; a rAF loop eases toward it every frame, so the turn is continuous
/// instead of one small step per poll.
const GAZE_TAU_MS = 110;

/// Below this an axis counts as "straight ahead", so a pointer sitting on
/// the character does not make it twitch. While an axis is in the dead zone the
/// last decided side is kept, so crossing the zone does not flip the face
/// between mirror frames.
const DIRECTION_DEADZONE = 6;

/// How often the pointer target is read. The OS call is cheap, but polling
/// it on every animation frame would burn a core for a 130-pixel window;
/// 50ms is fresh enough for a target that the rAF loop then eases toward.
const POINTER_POLL_MS = 50;

/// The blink cycle. The eyes squeeze (scale, not a swapped frame) roughly every
/// few seconds, short enough to read as a blink rather than a change of face.
const BLINK_INTERVAL_MS = 3400;
const BLINK_DURATION_MS = 130;

/// The nap is the resting state, not a timeout. The pet opens its eyes
/// asleep, wakes on any interaction, and drops back after this long without
/// one. It used to sulk here instead — a shut eye plus "不理你了" — and that
/// read as the default face, which the user rejected: a pet left alone should
/// look asleep, not offended.
const SLEEP_AFTER_MS = 6000;

export function initCompanion(): void {
  const surface = pick<HTMLElement>("#companion-surface");
  const bubble = pick<HTMLElement>("#companion-bubble");
  const mark = pick<HTMLElement>("#companion-drag");
  const pet = pick<HTMLElement>("#pet");

  let opening = false;
  let dragging = false;
  let thinking = false;
  let napping = true;
  let bubbleTimer: ReturnType<typeof setTimeout> | undefined;
  let moodTimer: ReturnType<typeof setTimeout> | undefined;
  let napTimer: ReturnType<typeof setTimeout> | undefined;

  /// The gaze direction the bot returns to after a blink. "blink" itself is
  /// never stored here: it is an overlay on the gaze, not a gaze.
  let look: Look = "neutral";
  /// Set while a blink is showing, so the poll does not fight it.
  let blinking = false;
  let lastSide: { h: "left" | "right"; v: "up" | "down" } = { h: "left", v: "up" };
  let lastDistance = Infinity;
  /// The gaze target (set by the poll) and the eased value (written by the
  /// rAF loop), normalised to ±1.
  let targetX = 0;
  let targetY = 0;
  let curX = 0;
  let curY = 0;
  let gazeRaf = 0;
  let gazeLast = 0;

  function setMood(mood: Mood, ms = 0): void {
    surface.dataset.mood = mood;
    pet.dataset.mood = mood;
    if (moodTimer) clearTimeout(moodTimer);
    if (ms > 0) {
      moodTimer = setTimeout(() => {
        if (!dragging && !napping) {
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

  /// The poll sets a target; the loop eases the written value toward it with
  /// an exponential decay, so every frame moves a little and the turn reads
  /// as one continuous glance instead of a step per poll. The loop stops
  /// itself once settled, so an idle pet costs nothing.
  function setTarget(x: number, y: number): void {
    targetX = x;
    targetY = y;
    if (!gazeRaf) {
      gazeLast = performance.now();
      gazeRaf = requestAnimationFrame(tick);
    }
  }

  function tick(now: number): void {
    const dt = Math.min(64, now - gazeLast);
    gazeLast = now;
    const k = 1 - Math.exp(-dt / GAZE_TAU_MS);
    curX += (targetX - curX) * k;
    curY += (targetY - curY) * k;
    if (Math.abs(targetX - curX) < 0.002 && Math.abs(targetY - curY) < 0.002) {
      curX = targetX;
      curY = targetY;
      gazeRaf = 0;
    } else {
      gazeRaf = requestAnimationFrame(tick);
    }
    pet.style.setProperty("--look-x", curX.toFixed(4));
    pet.style.setProperty("--look-y", curY.toFixed(4));
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

  /// The bot turns toward the pointer and its eyes follow, wherever on the
  /// screen the pointer is. The turn is a plain ratio, not an angle: how far
  /// the pointer is from the pet, in units of the pet's own size, decides how
  /// much the head turns, faded down at screen-scale distances.
  function aim(x: number, y: number, distance: number): void {
    const falloff = 1 / (1 + Math.max(0, distance - GAZE_REACH) / GAZE_FALLOFF);
    const nx = Math.max(-1, Math.min(1, x / GAZE_REACH)) * falloff;
    const ny = Math.max(-1, Math.min(1, y / GAZE_REACH)) * falloff;
    setTarget(nx, ny);
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
    wake();
    setMood("happy", 900);
    say(REACTIONS.happy);
    try {
      await invoke("companion_show_chat");
    } catch {
      fail();
    } finally {
      opening = false;
      restSoon();
    }
  }

  // ---- the blink cycle -------------------------------------------------
  //
  // Blinking is not decoration. A still-open stare is the single clearest
  // signal that an image is a picture, and this character is meant to read as
  // alive; so the face blinks on its own, and the eyes keep following the
  // pointer between blinks.
  function scheduleBlink(): void {
    setTimeout(() => {
      // A sleeping pet does not blink: the shut eye is already its face, and
      // swapping in the blink frame would only flash the lids.
      if (napping) { scheduleBlink(); return; }
      blinking = true;
      pet.dataset.look = "blink";
      setTimeout(() => {
        blinking = false;
        pet.dataset.look = look;
        scheduleBlink();
      }, BLINK_DURATION_MS);
    }, BLINK_INTERVAL_MS + Math.random() * 1800);
  }

  // ---- the nap ---------------------------------------------------------
  //
  // The nap is the resting state, not a punishment: the pet boots asleep, any
  // interaction wakes it — hover, click, drag, a generation starting — and it
  // drops back once that interaction is over. The eyes shut, the float slows
  // and one drool bead shows at the corner. Nothing is said on the way down: a
  // sleeping pet has no line, and a bubble would break the pose.
  function sleep(): void {
    if (napTimer) clearTimeout(napTimer);
    napping = true;
    blinking = false;
    setTarget(0, 0);
    setMood("sleeping");
    applyLook("sleepy");
  }

  /// Wakes the pet and cancels the countdown. The caller picks the mood it
  /// wakes into — curious on hover, happy on click, thinking on a run — and
  /// calls `restSoon` when that beat is over, so the pet returns to the nap
  /// instead of staring at the desktop.
  function wake(): void {
    if (napTimer) clearTimeout(napTimer);
    if (napping) {
      napping = false;
      if (look === "sleepy") applyLook("neutral");
      setMood("idle");
    }
  }

  function restSoon(ms = SLEEP_AFTER_MS): void {
    if (napTimer) clearTimeout(napTimer);
    napTimer = setTimeout(() => {
      if (dragging || opening || thinking || document.hidden) return;
      sleep();
    }, ms);
  }

  /// The pointer poll. It reads the OS cursor position and works out where
  /// the pointer is relative to the pet. `poll` owns the cadence: it pauses
  /// while the window is hidden and never overlaps two reads.
  async function readPointer(): Promise<void> {
    if (dragging || napping) return;
    const state = await invoke<PointerState>("companion_pointer");
    const scale = state.scaleFactor || 1;
    // The pet sits at the bottom of the window, under the bubble: its centre
    // is the pet box's centre, not the window's.
    const rect = pet.getBoundingClientRect();
    const centreX = state.windowX + (rect.left + rect.width / 2) * scale;
    const centreY = state.windowY + (rect.top + rect.height / 2) * scale;
    const dx = (state.screenX - centreX) / scale;
    const dy = (state.screenY - centreY) / scale;
    const distance = Math.hypot(dx, dy);

    if (!thinking && lastDistance >= STARTLE_FROM && distance < STARTLE_RADIUS) {
      setMood("surprised", 650);
      say(REACTIONS.surprised, 2200);
    }
    aim(dx, dy, distance);
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
      wake();
      setMood("curious");
      say(REACTIONS.curious);
    }
  });
  mark.addEventListener("pointerleave", () => {
    if (dragging) return;
    setMood("idle");
    restSoon();
  });

  mark.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || !desktop || dragging) return;
    event.preventDefault();
    dragging = true;
    lastDistance = Infinity;
    setTarget(0, 0);
    wake();
    setMood("dragging");
    say(REACTIONS.dragging);
    void invoke("window_drag").catch(() => { fail(); }).finally(() => {
      dragging = false;
      setMood("idle");
      restSoon();
    });
  });

  // The chat window publishes its generation state; the pet shows it even
  // when the chat overlay is hidden (e.g. a long answer running while the
  // user clicked away). A working pet is not an ignored pet: the sulk
  // countdown restarts on both edges.
  void listen<boolean>("warden:thinking", (event) => {
    thinking = event.payload;
    if (thinking) wake();
    setMood(thinking ? "thinking" : "idle");
    if (!thinking) restSoon();
  }).catch(() => { /* plain browser: no backend events */ });

  scheduleBlink();
  // The resting face is the nap: it opens its eyes asleep and wakes on the
  // first sign of interest, so a pet nobody is looking at never stares.
  sleep();
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (desktop && !reducedMotion) {
    poll(readPointer, POINTER_POLL_MS);
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) {
        lastDistance = Infinity;
        wake();
        restSoon();
      }
    });
  }
}
