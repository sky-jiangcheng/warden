import { describe, expect, it } from "vitest";
import { errorCode, errorHint } from "./ui";

// Phase 6 open question 8, option 2: the IPC boundary sends a stable
// `{ code, message }`, and the UI branches on the code. These two helpers are
// the whole of that contract on the frontend side, so they get a test rather
// than being trusted to be obvious.

describe("errorCode", () => {
  it("reads the code from a structured rejection", () => {
    expect(errorCode({ code: "AUTHENTICATION", message: "401" })).toBe("AUTHENTICATION");
    expect(errorCode({ code: "NOT_INSTALLED", message: "missing", data: {} })).toBe("NOT_INSTALLED");
  });

  it("returns null for anything that is not a structured error", () => {
    expect(errorCode(null)).toBeNull();
    expect(errorCode(undefined)).toBeNull();
    expect(errorCode("a plain string")).toBeNull();
    expect(errorCode(new Error("boom"))).toBeNull();
    expect(errorCode({ message: "no code" })).toBeNull();
    expect(errorCode({ code: 42, message: "numeric code" })).toBeNull();
  });
});

describe("errorHint", () => {
  it("gives a different action per category", () => {
    // The acceptance criterion is that the UI can act on the category; each
    // of these must be a distinct, non-empty action.
    const hints = ["NOT_INSTALLED", "AUTHENTICATION", "CONNECTION", "TIMEOUT", "INVALID_INPUT"].map(
      (code) => errorHint(code),
    );
    expect(hints.every((hint) => hint.length > 0)).toBe(true);
    expect(new Set(hints).size).toBe(hints.length);
  });

  it("is empty for an unknown code so callers can append unconditionally", () => {
    expect(errorHint(null)).toBe("");
    expect(errorHint("SOME_NEW_CODE")).toBe("");
  });
});
