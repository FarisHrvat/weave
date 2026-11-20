export type TeamReport = {
  published: number;
  imported: number;
  removed: number;
  conflicts: string[];
};

export function describeTeamSync(report: TeamReport) {
  const parts = [
    report.published && `shared ${report.published}`,
    report.imported && `received ${report.imported}`,
    report.removed && `removed ${report.removed}`,
  ].filter(Boolean);
  const summary = parts.length
    ? `Team vault: ${parts.join(", ")} host(s).`
    : "Team vault is up to date.";
  return report.conflicts.length
    ? `${summary} Both you and a teammate changed ${report.conflicts.join(", ")}; the team's version was kept.`
    : summary;
}
