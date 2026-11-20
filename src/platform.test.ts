import { describe, expect, it } from "vitest";
import { detectOs } from "./platform";

describe("platform", () => {
  it("tells the three systems apart from the web view", () => {
    expect(
      detectOs(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
      ),
    ).toBe("mac");
    expect(
      detectOs(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/131.0",
      ),
    ).toBe("windows");
    expect(
      detectOs("Mozilla/5.0 (X11; Ubuntu; Linux x86_64) AppleWebKit/605.1.15"),
    ).toBe("linux");
  });
});
