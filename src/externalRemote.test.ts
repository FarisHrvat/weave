import { describe, expect, it } from "vitest";
import { isExternalRemote } from "./externalRemote";

describe("external remote detection", () => {
  it("matches cloud and self-hosted Bitbucket and Azure DevOps origins", () => {
    for (const origin of [
      "https://bitbucket.org/team/app.git",
      "git@bitbucket.org:team/app.git",
      "ssh://git@bitbucket.org/team/app.git",
      "https://dev.azure.com/org/project/_git/app",
      "https://tfs.example.test/tfs/DefaultCollection/Work/_git/App",
      "https://git.example.test/bitbucket/scm/OPS/tools.git",
      "ssh://git@git.example.test:7999/ops/tools.git",
    ])
      expect(isExternalRemote(origin)).toBe(true);
  });

  it("leaves GitHub, GitLab and plain servers to the other panel", () => {
    for (const origin of [
      "https://github.com/a/b.git",
      "git@gitlab.com:a/b.git",
      "https://gitlab.example.test/group/sub/repo.git",
      "ssh://git@github.com/a/b.git",
      "",
    ])
      expect(isExternalRemote(origin)).toBe(false);
  });
});
