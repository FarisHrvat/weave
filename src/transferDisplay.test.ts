import { describe, expect, it } from "vitest";
import { remainingTime, sortTransfers, updateRate } from "./transferDisplay";
import type { TransferRecord } from "./types";

const transfer = (id: string, status: string, queueOrder: number) =>
  ({
    id,
    status,
    queueOrder,
    createdAt: queueOrder,
    bytesDone: 0,
    totalBytes: 1000,
  }) as TransferRecord;

describe("transfer activity", () => {
  it("keeps queued transfers in their saved order ahead of history", () => {
    const records = [
      transfer("done", "done", 1),
      transfer("second", "queued", 2),
      transfer("first", "queued", 1),
    ];
    expect(sortTransfers(records).map((item) => item.id)).toEqual([
      "first",
      "second",
      "done",
    ]);
  });

  it("estimates remaining time from observed progress", () => {
    const start = transfer("copy", "running", 1);
    const first = updateRate(undefined, start, 1000);
    const progress = { ...start, bytesDone: 500 };
    const second = updateRate(first, progress, 2000);
    expect(remainingTime(progress, second)).toBe("~1s left");
    expect(updateRate(second, { ...progress, status: "paused" }, 3000)).toBe(
      undefined,
    );
  });
});
