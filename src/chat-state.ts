// chat-state.ts — the chat state-machine decisions as pure functions.
//
// Rationale (roadmap "engineering gap 1", product.md open question 6): the
// four behaviours that guard the chat flow — send blocking, stale-response
// dropping, thread-list overwrite rules and draft retention — used to be
// inline conditions inside `chat.ts`, reachable only through the DOM and a
// live Tauri IPC. They are the parts most likely to regress and the least
// likely to be noticed: a dropped draft or a resurrected stale reply is
// silent. Extracted here, every rule can be tested directly; `chat.ts`
// calls these functions, so the tests exercise the production path, not a
// copy that can drift.

/** The composer grace window after `compositionend`. Some WebViews fire a
 *  synthetic Enter immediately after composition ends; sending inside the
 *  window double-sends the half-typed IME text. Must stay in sync with the
 *  value `chat.ts` passes to `sendBlocked`. */
export const COMPOSITION_GRACE_MS = 80;

/** True when a send attempt must be ignored.
 *  - composing: the IME still owns the text;
 *  - within the post-composition grace: the Enter belongs to the composition;
 *  - busy: one reply at a time per topic (Enter and submit cannot double-send);
 *  - empty text: nothing to send. */
export function sendBlocked(args: {
  composing: boolean;
  msSinceCompositionEnd: number;
  busy: boolean;
  text: string;
}): boolean {
  return (
    args.composing ||
    args.msSinceCompositionEnd < COMPOSITION_GRACE_MS ||
    args.busy ||
    args.text.trim() === ""
  );
}

/** True when an in-flight `conversation_read` answer is stale and must be
 *  dropped: the revision moved while the request was in flight (a local
 *  rename/send bumped it), or the topic became busy (a reply is streaming —
 *  server history must not cut it off mid-token). */
export function readIsStale(args: {
  revisionAtRequest: number;
  revisionNow: number;
  busy: boolean;
}): boolean {
  return args.revisionNow !== args.revisionAtRequest || args.busy;
}

/** True when a thread row from `conversation_list` may overwrite the local
 *  topic: the revision has not moved since the snapshot (a local rename can
 *  finish while the list is in flight), the topic is not busy, and the
 *  server is not older than what we already know. */
export function shouldAdoptThread(args: {
  revisionUnchanged: boolean;
  busy: boolean;
  hasLocalThread: boolean;
  serverUpdatedAt: string;
  localUpdatedAt: string;
}): boolean {
  if (!args.revisionUnchanged || args.busy) return false;
  return !args.hasLocalThread || args.serverUpdatedAt >= args.localUpdatedAt;
}

/** The draft after a send finishes.
 *  - failed: keep everything — the whole point of "发送未完成，草稿已保留";
 *  - succeeded but the user already edited: keep the new work untouched;
 *  - succeeded and untouched: clear it.
 *  Returning the current draft means "leave it as it is". */
export function draftAfterSend(args: {
  draftNow: string;
  draftAtSend: string;
  failed: boolean;
}): string {
  if (args.failed) return args.draftNow;
  return args.draftNow === args.draftAtSend ? "" : args.draftNow;
}
