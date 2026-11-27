import { describe, expect, it } from "vitest";
import {
  COMPOSITION_GRACE_MS,
  contextEditable,
  draftAfterSend,
  outline,
  readIsStale,
  savedThreadLabel,
  sendBlocked,
  shouldAdoptThread,
  splitThinking,
  stripThinking,
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

describe("savedThreadLabel — a shared thread says whether it earns a handoff", () => {
  it("marks a repository-scoped thread as saved", () => {
    expect(savedThreadLabel("/Users/me/project")).toBe("由 Cinder 保存对话");
  });

  it("flags an unscoped thread as not writing a checkpoint", () => {
    expect(savedThreadLabel(null)).toBe("由 Cinder 保存对话 · 未挂仓库，不写交接");
    expect(savedThreadLabel("")).toBe("由 Cinder 保存对话 · 未挂仓库，不写交接");
  });
});

describe("contextEditable — when the scope dropdown stays unlocked", () => {
  it("allows choosing the scope on an idle topic", () => {
    expect(contextEditable({ busy: false })).toBe(true);
  });

  it("keeps a topic that already carries messages editable", () => {
    // The scope is read at generation time, not frozen at the first message:
    // re-attaching only changes which checkpoint the NEXT reply earns.
    expect(contextEditable({ busy: false })).toBe(true);
  });

  it("locks a topic while a reply is streaming", () => {
    expect(contextEditable({ busy: true })).toBe(false);
  });
});

describe("outline — the collapsed panel's one-line summary", () => {
  it("takes the first non-empty line, trimmed", () => {
    expect(outline("\n\n  M a.txt\nM b.txt")).toBe("M a.txt");
  });

  it("caps a long first line so the collapsed state stays one line", () => {
    // A collapsed panel taller than the expanded one defeats the point.
    const got = outline("x".repeat(500), 10);
    expect(got).toBe("xxxxxxxxxx…");
    expect(got.length).toBe(11);
  });

  it("says so when there is nothing to show", () => {
    expect(outline("")).toBe("空");
    expect(outline("   \n\t\n")).toBe("空");
  });
});

describe("stripThinking", () => {
  it("removes a closed reasoning block", () => {
    expect(stripThinking("<think>plan: read the note first</think>The retry budget was raised."))
      .toBe("The retry budget was raised.");
  });

  it("recognises both spellings, any case, and reasoning", () => {
    expect(stripThinking("<thinking>x</thinking>A.")).toBe("A.");
    expect(stripThinking("<THINK>x</THINK>A.")).toBe("A.");
    expect(stripThinking("<reasoning>x</reasoning>A.")).toBe("A.");
  });

  it("drops an unterminated block to the end (a streaming answer)", () => {
    expect(stripThinking("Answer so far.\n<think>still thinking")).toBe("Answer so far.");
  });

  it("removes a stray tag and keeps the text around it", () => {
    expect(stripThinking("Answer.</think>")).toBe("Answer.");
  });
});

describe("splitThinking", () => {
  it("splits the notes from the answer", () => {
    expect(splitThinking("<think>plan: read the note first</think>The retry budget was raised."))
      .toEqual({ thoughts: ["plan: read the note first"], body: "The retry budget was raised." });
  });

  it("treats an unterminated block as reasoning (a streaming answer)", () => {
    expect(splitThinking("Answer so far.\n<think>still thinking"))
      .toEqual({ thoughts: ["still thinking"], body: "Answer so far." });
  });

  it("keeps several blocks in order", () => {
    expect(splitThinking("<think>a</think>Mid.<think>b</think>End."))
      .toEqual({ thoughts: ["a", "b"], body: "Mid.End." });
  });
});
