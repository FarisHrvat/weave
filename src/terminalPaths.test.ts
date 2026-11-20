import { describe, expect, it } from "vitest";
import { findPathLinks, parentFolder, resolvePath } from "./terminalPaths";
import {
  completionLines,
  completionScript,
  currentWord,
} from "./terminalCompletion";

describe("terminal file paths", () => {
  it("finds paths, home paths and file:line references", () => {
    const links = findPathLinks(
      "error at src/main.rs:12:5 and ~/notes.txt, see /var/log/system.log.",
    );
    expect(links.map((link) => [link.path, link.line])).toEqual([
      ["src/main.rs", 12],
      ["~/notes.txt", undefined],
      ["/var/log/system.log", undefined],
    ]);
    expect(
      "error at src/main.rs:12:5".slice(links[0].start, links[0].end),
    ).toBe("src/main.rs:12:5");
  });

  it("leaves web links and lone slashes alone", () => {
    expect(findPathLinks("open https://example.com/a/b.html now")).toEqual([]);
    expect(findPathLinks("a / b")).toEqual([]);
  });

  it("resolves paths against the current folder", () => {
    expect(resolvePath("../lib/x.ts", "/srv/app/src", "/home/ada")).toBe(
      "/srv/app/lib/x.ts",
    );
    expect(resolvePath("~/a", "/tmp", "/home/ada")).toBe("/home/ada/a");
    expect(parentFolder("/srv/app/x.ts")).toBe("/srv/app");
    expect(parentFolder("/x")).toBe("/");
  });
});

describe("host completion", () => {
  it("knows which word is being completed", () => {
    expect(currentWord("gi")).toEqual({ word: "gi", command: true });
    expect(currentWord("cd src/ma")).toEqual({
      word: "src/ma",
      command: false,
    });
    expect(currentWord("ls | gr")).toEqual({ word: "gr", command: true });
  });

  it("builds a quoted script and full lines", () => {
    const script = completionScript("it's", false);
    expect(script).toContain(`_ 'it'\\''s' f`);
    expect(
      completionLines(
        "vim src/ma",
        "src/ma",
        "src/main.rs\nsrc/make file.ts\nother\n",
      ),
    ).toEqual(["vim src/main.rs", "vim src/make\\ file.ts"]);
  });
});
