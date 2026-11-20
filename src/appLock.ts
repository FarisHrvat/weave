export type AppLockSettings = {
  enabled: boolean;
  idleMinutes: number;
  /** True where unlocking asks for Weave's own password (not on a Mac). */
  usesPassword?: boolean;
};

/** Whether a Weave left alone since `lastActive` should lock now. */
export function idleLockDue(
  settings: AppLockSettings | undefined,
  lastActive: number,
  now: number,
): boolean {
  if (!settings?.enabled || settings.idleMinutes <= 0) return false;
  return now - lastActive >= settings.idleMinutes * 60_000;
}
