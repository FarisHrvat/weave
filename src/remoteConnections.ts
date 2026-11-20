export type RemoteConnectionKind =
  "telnet" | "serial" | "mosh" | "x11" | "vnc" | "rdp";

/** A non-SSH connection. Mosh and X11 reuse a saved SSH host profile. */
export type RemoteConnection = {
  kind: RemoteConnectionKind;
  name: string;
  profileId?: string;
  host?: string;
  port?: number;
  username?: string;
  device?: string;
  baud?: number;
  command?: string;
};

export type SavedRemoteConnection = RemoteConnection & { id: string };

export const terminalConnectionKinds: RemoteConnectionKind[] = [
  "telnet",
  "serial",
  "mosh",
  "x11",
];

export const connectionKindLabels: Record<RemoteConnectionKind, string> = {
  telnet: "Telnet",
  serial: "Serial",
  mosh: "Mosh",
  x11: "X11 over SSH",
  vnc: "VNC",
  rdp: "RDP",
};

export const defaultPorts: Partial<Record<RemoteConnectionKind, number>> = {
  telnet: 23,
  vnc: 5900,
  rdp: 3389,
};

export const serialBaudRates = [
  300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800,
  921600,
];

export function isTerminalConnection(connection: RemoteConnection) {
  return terminalConnectionKinds.includes(connection.kind);
}

const hostPattern = /^[A-Za-z0-9._:[\]-]{1,253}$/;

/** Returns an error message, or an empty string when the connection is usable. */
export function validateConnection(connection: RemoteConnection): string {
  if (!connection.name.trim() || connection.name.length > 80)
    return "Enter a name up to 80 characters.";
  switch (connection.kind) {
    case "telnet":
    case "vnc":
    case "rdp":
      if (
        !connection.host ||
        !hostPattern.test(connection.host) ||
        connection.host.startsWith("-")
      )
        return "Enter a host name or IP address without spaces.";
      if (
        !Number.isInteger(connection.port) ||
        connection.port! < 1 ||
        connection.port! > 65535
      )
        return "Enter a port between 1 and 65535.";
      return "";
    case "serial":
      if (!connection.device || !/^\/dev\/(cu|tty)\./.test(connection.device))
        return "Choose a serial device.";
      if (!serialBaudRates.includes(connection.baud ?? 0))
        return "Choose a serial speed.";
      return "";
    case "mosh":
    case "x11":
      return connection.profileId ? "" : "Choose a saved SSH host.";
    default:
      return "Choose a connection type.";
  }
}

export function describeConnection(
  connection: RemoteConnection,
  profileHost?: string,
): string {
  switch (connection.kind) {
    case "serial":
      return `${connection.device} · ${connection.baud} baud`;
    case "mosh":
    case "x11":
      return profileHost ?? "SSH host";
    default:
      return `${connection.host}:${connection.port}`;
  }
}

const storageKey = "weave-remote-connections";
const maxConnections = 100;

export function readRemoteConnections(
  storage: Pick<Storage, "getItem">,
): SavedRemoteConnection[] {
  const saved = storage.getItem(storageKey);
  if (!saved) return [];
  try {
    const parsed: unknown = JSON.parse(saved);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter(
        (item): item is SavedRemoteConnection =>
          !!item &&
          typeof item === "object" &&
          typeof item.id === "string" &&
          typeof item.kind === "string" &&
          typeof item.name === "string" &&
          validateConnection(item) === "",
      )
      .slice(0, maxConnections);
  } catch {
    return [];
  }
}

export function writeRemoteConnections(
  storage: Pick<Storage, "setItem">,
  connections: SavedRemoteConnection[],
) {
  if (connections.length > maxConnections)
    throw new Error(`Save up to ${maxConnections} connections.`);
  storage.setItem(storageKey, JSON.stringify(connections));
}

/** Adds imported sessions that are valid and not already saved. */
export function mergeImportedConnections(
  saved: SavedRemoteConnection[],
  imported: RemoteConnection[],
  makeId: () => string,
): { connections: SavedRemoteConnection[]; added: number } {
  const key = (item: RemoteConnection) =>
    `${item.kind}|${(item.host ?? "").toLowerCase()}|${item.port ?? ""}`;
  const known = new Set(saved.map(key));
  const connections = [...saved];
  let added = 0;
  for (const item of imported) {
    if (connections.length >= maxConnections) break;
    if (known.has(key(item)) || validateConnection(item)) continue;
    known.add(key(item));
    connections.push({ ...item, id: makeId() });
    added += 1;
  }
  return { connections, added };
}
