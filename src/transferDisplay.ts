import type { TransferRecord } from "./types";

export type RateSample = {
  bytesDone: number;
  atMs: number;
  bytesPerSecond: number;
};

const active = (status: string) =>
  status === "queued" || status === "running" || status === "paused";

export function sortTransfers(records: TransferRecord[]): TransferRecord[] {
  return [...records].sort((a, b) => {
    if (active(a.status) !== active(b.status)) return active(a.status) ? -1 : 1;
    if (active(a.status)) return a.queueOrder - b.queueOrder;
    return b.createdAt - a.createdAt;
  });
}

export function updateRate(
  previous: RateSample | undefined,
  record: TransferRecord,
  atMs: number,
): RateSample | undefined {
  if (record.status !== "running") return undefined;
  if (!previous || record.bytesDone < previous.bytesDone)
    return { bytesDone: record.bytesDone, atMs, bytesPerSecond: 0 };
  const elapsed = atMs - previous.atMs;
  if (elapsed < 200) return previous;
  const bytesPerSecond =
    ((record.bytesDone - previous.bytesDone) * 1000) / elapsed;
  return {
    bytesDone: record.bytesDone,
    atMs,
    bytesPerSecond: previous.bytesPerSecond
      ? previous.bytesPerSecond * 0.4 + bytesPerSecond * 0.6
      : bytesPerSecond,
  };
}

export function remainingTime(
  record: TransferRecord,
  sample: RateSample | undefined,
): string | null {
  if (
    record.status !== "running" ||
    !sample?.bytesPerSecond ||
    record.totalBytes <= record.bytesDone
  )
    return null;
  const seconds = Math.ceil(
    (record.totalBytes - record.bytesDone) / sample.bytesPerSecond,
  );
  if (seconds < 60) return `~${seconds}s left`;
  if (seconds < 3600) return `~${Math.ceil(seconds / 60)}m left`;
  return `~${Math.ceil(seconds / 3600)}h left`;
}
