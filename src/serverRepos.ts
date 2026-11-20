import type { Profile } from "./types";

/** Builds the clone URL for a repository on an SSH profile's host. */
export function repositoryCloneUrl(profile: Profile | undefined, path: string) {
  if (!profile) return path;
  const remotePath = path.startsWith("~/")
    ? `~${profile.username}/${path.slice(2)}`
    : path.replace(/^\/+/, "");
  return `ssh://${profile.username}@${profile.host}:${profile.port}/${remotePath}`;
}
