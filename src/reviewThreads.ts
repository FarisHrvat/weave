import type { ForgeReviewThread } from "./types";

export function mergeReviewThreads(
  current: ForgeReviewThread[],
  incoming: ForgeReviewThread[],
): ForgeReviewThread[] {
  const merged = current.map((thread) => ({
    ...thread,
    comments: [...thread.comments],
  }));
  const locations = new Map(merged.map((thread, index) => [thread.id, index]));
  for (const thread of incoming) {
    const index = locations.get(thread.id);
    if (index === undefined) {
      locations.set(thread.id, merged.length);
      merged.push(thread);
      continue;
    }
    const existing = merged[index];
    const seen = new Set(existing.comments.map((comment) => comment.id));
    for (const comment of thread.comments) {
      if (!seen.has(comment.id)) {
        existing.comments.push(comment);
        seen.add(comment.id);
      }
    }
    if (!existing.path) existing.path = thread.path;
    if (existing.line === null) existing.line = thread.line;
    if (thread.providerId) {
      existing.providerId = thread.providerId;
      existing.resolved = thread.resolved;
    }
  }
  return merged;
}
