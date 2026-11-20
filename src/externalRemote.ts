/**
 * True for origins handled by the Bitbucket / Azure DevOps panel, using the
 * same rules as the backend: Bitbucket Cloud, Azure DevOps (cloud and
 * Server), and Bitbucket Data Center.
 */
export function isExternalRemote(origin: string): boolean {
  const remote = origin.trim();
  if (
    /^(https:\/\/(bitbucket\.org|dev\.azure\.com)\/|git@(bitbucket\.org|ssh\.dev\.azure\.com):|ssh:\/\/git@(bitbucket\.org|ssh\.dev\.azure\.com)\/)/.test(
      remote,
    )
  )
    return true;
  let url: URL;
  try {
    url = new URL(remote);
  } catch {
    return false;
  }
  const parts = url.pathname.split("/").filter(Boolean);
  if (url.protocol === "https:" && !url.username)
    return (
      (parts.length >= 4 && parts[parts.length - 2] === "_git") ||
      (parts.includes("scm") && parts.length - parts.indexOf("scm") === 3)
    );
  // Bitbucket Data Center serves SSH on its own port (7999 by default).
  return url.protocol === "ssh:" && !!url.port && parts.length === 2;
}
