import { describe, expect, it } from "vitest";
import {
  COMPOSITION_GRACE_MS,
  draftAfterSend,
  readIsStale,
  sendBlocked,
  shouldAdoptThread,
} from "./chat-state";

// Acceptance criteria (product.md open question 6, roadmap engineering
// gap 1): four paths, automated —
//   1. concurrent sends on one topic keep exactly one reply;
//   2. a response whose revision fell behind is dropped;
//   3. no send fires while the IME is composing;
//   4. a failed request keeps the draft.

const idleSend = { composing: false, msSinceCompositionEnd: 10_000, busy: false, text: "hi" };

describe("sendBlocked — concurrency and emptiness", () => {
  it("does not block a normal send", () => {
    expect(sendBlocked(idleSend)).toBe(false);
  });

  it("blocks a second send while the topic is replying", () => {
    // The point of `busy` set before the first await: Enter and submit
    // cannot double-send, so the one in-flight reply is the only one kept.
    expect(sendBlocked({ ...idleSend, busy: true })).toBe(true);
  });

  it("blocks whitespace-only text", () => {
    expect(sendBlocked({ ...idleSend, text: "   \n " })).toBe(true);
  });
});

describe("sendBlocked — the IME guard", () => {
  it("never sends while composition is active", () => {
    expect(sendBlocked({ ...idleSend, composing: true })).toBe(true);
  });

  it("swallows the synthetic Enter just after compositionend", () => {
    expect(sendBlocked({ ...idleSend, composing: false, msSinceCompositionEnd: COMPOSITION_GRACE_MS - 1 })).toBe(true);
    expect(sendBlocked({ ...idleSend, composing: false, msSinceCompositionEnd: COMPOSITION_GRACE_MS })).toBe(false);
  });

  it("exposes the grace window that chat.ts's keydown guard shares", () => {
    // If these two values ever drift, the Enter guard and the send guard
    // disagree about when a composition has really ended.
    expect(COMPOSITION_GRACE_MS).toBe(80);
  });
});

describe("readIsStale — dropping responses that fell behind", () => {
  it("drops a read whose revision moved while in flight", () => {
    // A local rename or send bumped the revision; the polled history is from
    // before it, so applying it would resurrect pre-rename state.
    expect(readIsStale({ revisionAtRequest: 3, revisionNow: 4, busy: false })).toBe(true);
  });

  it("drops a read when the topic became busy mid-flight", () => {
    // A streaming reply must not be cut off by server history arriving late.
    expect(readIsStale({ revisionAtRequest: 3, revisionNow: 3, busy: true })).toBe(true);
  });

  it("applies the read when nothing moved", () => {
    expect(readIsStale({ revisionAtRequest: 3, revisionNow: 3, busy: false })).toBe(false);
  });
});

describe("shouldAdoptThread — one list update does not undo local work", () => {
  it("refuses to overwrite a topic whose revision moved during the fetch", () => {
    expect(
      shouldAdoptThread({
        revisionUnchanged: false, busy: false, hasLocalThread: true,
        serverUpdatedAt: "2026-11-12T00:00:00Z", localUpdatedAt: "2026-11-11T00:00:00Z",
      }),
    ).toBe(false);
  });

  it("refuses to overwrite a topic that is replying", () => {
    expect(
      shouldAdoptThread({
        revisionUnchanged: true, busy: true, hasLocalThread: true,
        serverUpdatedAt: "2026-11-12T00:00:00Z", localUpdatedAt: "2026-11-11T00:00:00Z",
      }),
    ).toBe(false);
  });

  it("refuses an older server row over a newer local one", () => {
    expect(
      shouldAdoptThread({
        revisionUnchanged: true, busy: false, hasLocalThread: true,
        serverUpdatedAt: "2026-11-10T00:00:00Z", localUpdatedAt: "2026-11-11T00:00:00Z",
      }),
    ).toBe(false);
  });

  it("adopts a row that is at least as new", () => {
    expect(
      shouldAdoptThread({
        revisionUnchanged: true, busy: false, hasLocalThread: true,
        serverUpdatedAt: "2026-11-12T00:00:00Z", localUpdatedAt: "2026-11-11T00:00:00Z",
      }),
    ).toBe(true);
  });

  it("adopts a thread the topic has never seen", () => {
    expect(
      shouldAdoptThread({
        revisionUnchanged: true, busy: false, hasLocalThread: false,
        serverUpdatedAt: "any", localUpdatedAt: "",
      }),
    ).toBe(true);
  });
});

describe("draftAfterSend — failure keeps the draft", () => {
  it("keeps the draft when the send failed", () => {
    expect(draftAfterSend({ draftNow: "work in progress", draftAtSend: "sent text", failed: true })).toBe("work in progress");
  });

  it("keeps a re-typed draft when the send succeeded", () => {
    // The user may already be composing the next message while the reply
    // streams; clearing that work would erase unsent text.
    expect(draftAfterSend({ draftNow: "next question", draftAtSend: "sent text", failed: false })).toBe("next question");
  });

  it("clears the draft after an untouched successful send", () => {
    expect(draftAfterSend({ draftNow: "sent text", draftAtSend: "sent text", failed: false })).toBe("");
  });
});
