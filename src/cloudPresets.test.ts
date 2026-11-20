import { describe, expect, it } from "vitest";
import { cloudBucketUrl, presetRegion } from "./cloudPresets";

describe("S3 cloud presets", () => {
  it("uses each provider's path-style bucket endpoint", () => {
    expect(cloudBucketUrl("r2", "auto", "backup", "a".repeat(32))).toBe(
      `https://${"a".repeat(32)}.r2.cloudflarestorage.com/backup`,
    );
    expect(cloudBucketUrl("b2", "us-west-004", "backup")).toBe(
      "https://s3.us-west-004.backblazeb2.com/backup",
    );
    expect(cloudBucketUrl("wasabi", "eu-central-1", "backup")).toBe(
      "https://s3.eu-central-1.wasabisys.com/backup",
    );
    expect(cloudBucketUrl("spaces", "nyc3", "backup")).toBe(
      "https://nyc3.digitaloceanspaces.com/backup",
    );
    expect(presetRegion("r2")).toBe("auto");
  });

  it("rejects malformed bucket, account and region fields", () => {
    expect(() => cloudBucketUrl("r2", "auto", "backup", "wrong")).toThrow(
      "account ID",
    );
    expect(() => cloudBucketUrl("b2", "us-west-004", "../secret")).toThrow(
      "bucket name",
    );
    expect(() =>
      cloudBucketUrl("spaces", "nyc3.example.test", "backup"),
    ).toThrow("region code");
  });
});
