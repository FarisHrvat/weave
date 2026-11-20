import type { Profile } from "./types";

/** How a saved connection's address is shown: user@host, or the URL alone. */
export function profileAddress(
  profile: Pick<Profile, "host" | "username" | "protocol">,
): string {
  if (profile.host.includes("://") || profile.protocol === "s3")
    return profile.host;
  if (["dropbox", "gdrive", "onedrive"].includes(profile.protocol))
    return profile.username;
  return profile.username
    ? `${profile.username}@${profile.host}`
    : profile.host;
}
