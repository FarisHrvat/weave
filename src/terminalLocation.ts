export function parseOsc7(data: string): string | null {
  try {
    const url = new URL(data);
    if (
      url.protocol !== "file:" ||
      url.username ||
      url.password ||
      url.search ||
      url.hash
    )
      return null;
    const path = decodeURIComponent(url.pathname);
    return path.startsWith("/") && !path.includes("\0") ? path : null;
  } catch {
    return null;
  }
}
