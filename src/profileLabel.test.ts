import { describe, expect, it } from "vitest";
import { profileAddress } from "./profileLabel";

describe("profileAddress", () => {
  it("shows user@host for servers and the URL alone for storage", () => {
    expect(
      profileAddress({ host: "web-01", username: "deploy", protocol: "ssh" }),
    ).toBe("deploy@web-01");
    expect(
      profileAddress({
        host: "https://s3.eu-central-1.amazonaws.com/assets",
        username: "AKIAEXAMPLE",
        protocol: "s3",
      }),
    ).toBe("https://s3.eu-central-1.amazonaws.com/assets");
    expect(
      profileAddress({
        host: "dropbox.com",
        username: "ada@example.com",
        protocol: "dropbox",
      }),
    ).toBe("ada@example.com");
  });
});
