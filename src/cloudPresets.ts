export type CloudPreset = "custom" | "aws" | "r2" | "b2" | "wasabi" | "spaces";

export function cloudBucketUrl(
  preset: CloudPreset,
  region: string,
  bucket: string,
  account = "",
): string {
  if (preset === "custom")
    throw new Error("Enter the custom S3 bucket URL directly.");
  if (
    !/^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/.test(bucket) ||
    bucket.includes("..")
  ) {
    throw new Error(
      "Enter a bucket name of 3–63 lowercase letters, numbers, dots, or dashes.",
    );
  }
  if (preset === "r2") {
    if (!/^[a-f0-9]{32}$/i.test(account)) {
      throw new Error(
        "Enter the 32-character Cloudflare account ID from your R2 dashboard.",
      );
    }
    return `https://${account.toLowerCase()}.r2.cloudflarestorage.com/${bucket}`;
  }
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(region)) {
    throw new Error("Enter the region code shown by your cloud provider.");
  }
  const host = {
    aws: `s3.${region}.amazonaws.com`,
    b2: `s3.${region}.backblazeb2.com`,
    wasabi: `s3.${region}.wasabisys.com`,
    spaces: `${region}.digitaloceanspaces.com`,
  }[preset];
  return `https://${host}/${bucket}`;
}

export function presetRegion(preset: CloudPreset): string {
  return {
    custom: "us-east-1",
    aws: "us-east-1",
    r2: "auto",
    b2: "us-west-004",
    wasabi: "eu-central-1",
    spaces: "nyc3",
  }[preset];
}
