import { describe, expect, it } from "vitest";
import { fillSnippet, pinnedSnippets, snippetVariables } from "./snippets";

describe("terminal snippets", () => {
  it("finds each variable once and fills its occurrences", () => {
    const body = "sbatch {{script}} --job-name={{name}} && echo {{name}}";
    expect(snippetVariables(body)).toEqual(["script", "name"]);
    expect(fillSnippet(body, { script: "train.sh", name: "experiment" })).toBe(
      "sbatch train.sh --job-name=experiment && echo experiment",
    );
  });

  it("explains missing variable values", () => {
    expect(() => fillSnippet("echo {{project}}", {})).toThrow(
      "Enter a value for project.",
    );
  });

  it("shows pinned global and matching host commands", () => {
    const snippets = [
      {
        id: "global",
        name: "Global",
        body: "pwd",
        profileId: null,
        pinned: true,
      },
      { id: "nas", name: "NAS", body: "df -h", profileId: "nas", pinned: true },
      {
        id: "other",
        name: "Other",
        body: "uptime",
        profileId: "other",
        pinned: true,
      },
      {
        id: "hidden",
        name: "Hidden",
        body: "ls",
        profileId: null,
        pinned: false,
      },
    ];
    expect(pinnedSnippets(snippets, "nas").map((item) => item.id)).toEqual([
      "global",
      "nas",
    ]);
    expect(pinnedSnippets(snippets).map((item) => item.id)).toEqual(["global"]);
  });
});
