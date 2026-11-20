import { describe, expect, it } from "vitest";
import { repositoryCloneUrl } from "./serverRepos";
import type { Profile } from "./types";

const profile: Profile = {
  id: "1",
  name: "NAS",
  host: "nas.local",
  protocol: "ssh",
  port: 22,
  username: "git",
  keyPath: null,
  jumpHost: null,
  color: "#000",
  favorite: false,
  tlsPinSha256: null,
  s3Region: null,
};

describe("server repository clone URLs", () => {
  it("matches the backend's URL format", () => {
    expect(repositoryCloneUrl(profile, "/srv/git/app.git")).toBe(
      "ssh://git@nas.local:22/srv/git/app.git",
    );
    expect(repositoryCloneUrl(profile, "~/git/app.git")).toBe(
      "ssh://git@nas.local:22/~git/git/app.git",
    );
    expect(repositoryCloneUrl(undefined, "/Users/me/app.git")).toBe(
      "/Users/me/app.git",
    );
  });
});
