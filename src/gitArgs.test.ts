import { describe, expect, it } from "vitest";
import { splitArgs } from "./gitArgs";

describe("custom Git command arguments", () => {
  it("splits on spaces and keeps quoted text together", () => {
    expect(splitArgs("log --oneline -5")).toEqual(["log", "--oneline", "-5"]);
    expect(splitArgs(`commit -m "fix the bug" --author='A B <a@b>'`)).toEqual([
      "commit",
      "-m",
      "fix the bug",
      "--author=A B <a@b>",
    ]);
    expect(splitArgs(`tag -m "" v1`)).toEqual(["tag", "-m", "", "v1"]);
    expect(() => splitArgs(`log "unclosed`)).toThrow("not closed");
  });
});
