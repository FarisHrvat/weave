import { describe, expect, it } from "vitest";
import { describeTeamSync } from "./teamVault";

describe("describeTeamSync", () => {
  it("summarizes changes and conflicts", () => {
    expect(
      describeTeamSync({
        published: 0,
        imported: 0,
        removed: 0,
        conflicts: [],
      }),
    ).toBe("Team vault is up to date.");
    expect(
      describeTeamSync({
        published: 1,
        imported: 2,
        removed: 0,
        conflicts: ["web"],
      }),
    ).toBe(
      "Team vault: shared 1, received 2 host(s). Both you and a teammate changed web; the team's version was kept.",
    );
  });
});
