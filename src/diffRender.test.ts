import { describe, expect, it } from "vitest";
import {
  highlight,
  isImage,
  languageFor,
  pairedWordDiffs,
  segmentsHtml,
  splitFiles,
  wordDiff,
} from "./diffRender";

describe("languageFor", () => {
  it("knows common extensions and special file names", () => {
    expect(languageFor("src/main.rs")).toBe("rust");
    expect(languageFor("App.TSX")).toBe("typescript");
    expect(languageFor("docker/Dockerfile")).toBe("dockerfile");
    expect(languageFor("README")).toBeUndefined();
    expect(languageFor(".gitignore")).toBeUndefined();
  });

  it("recognises images", () => {
    expect(isImage("logo.PNG")).toBe(true);
    expect(isImage("logo.txt")).toBe(false);
  });
});

describe("highlight", () => {
  it("escapes text that has no language", () => {
    expect(highlight("<b>&</b>")).toBe("&lt;b&gt;&amp;&lt;/b&gt;");
  });

  it("marks up code and still escapes it", () => {
    const html = highlight('let x = "<a>";', "rust");
    expect(html).toContain("hljs-keyword");
    expect(html).not.toContain("<a>");
  });
});

describe("wordDiff", () => {
  it("marks only the words that changed", () => {
    const { before, after } = wordDiff(
      "const total = price * 2;",
      "const total = cost * 2;",
    );
    expect(before.filter((s) => s.changed).map((s) => s.text)).toEqual([
      "price",
    ]);
    expect(after.filter((s) => s.changed).map((s) => s.text)).toEqual(["cost"]);
    expect(after.map((s) => s.text).join("")).toBe("const total = cost * 2;");
  });

  it("falls back to the shared start and end for very long lines", () => {
    const long = "a ".repeat(400);
    const { after } = wordDiff(long + "x", long + "y");
    expect(after.map((s) => s.text).join("")).toBe(long + "y");
    expect(after.at(-1)).toEqual({ text: "y", changed: true });
  });

  it("renders marks around changed words", () => {
    expect(
      segmentsHtml([
        { text: "a ", changed: false },
        { text: "<b>", changed: true },
      ]),
    ).toBe("a <mark>&lt;b&gt;</mark>");
  });
});

describe("pairedWordDiffs", () => {
  it("pairs removed lines with the added lines after them", () => {
    const pairs = pairedWordDiffs([
      " context",
      "-let value = 1;",
      "-let other = 2;",
      "+let value = 3;",
      " end",
      "+only added",
    ]);
    expect([...pairs.keys()]).toEqual([1, 3]);
    expect(pairs.get(3)!.some((s) => s.changed && s.text === "3")).toBe(true);
  });

  it("leaves completely rewritten lines unmarked", () => {
    expect(pairedWordDiffs(["-alpha beta", "+gamma delta"]).size).toBe(0);
  });
});

describe("splitFiles", () => {
  it("separates a multi-file diff and names each file", () => {
    const files = splitFiles(
      "diff --git a/one.txt b/one.txt\n--- a/one.txt\n+++ b/one.txt\n@@ -1 +1 @@\n-a\n+b\n" +
        "diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n" +
        "diff --git a/pic.png b/pic.png\nBinary files a/pic.png and b/pic.png differ\n",
    );
    expect(files.map((file) => file.path)).toEqual([
      "one.txt",
      "gone.txt",
      "pic.png",
    ]);
    expect(files[0].body).toBe("@@ -1 +1 @@\n-a\n+b\n");
    expect(files[2].body).toBe("");
  });
});
