export type PathLink = {
  start: number;
  end: number;
  path: string;
  line?: number;
};

// Absolute, home-relative or ./ ../ paths, or relative paths with a slash
// and an extension, optionally followed by :line or :line:column.
const pathPattern =
  /(?:^|[\s'"`(=[])((?:~|\.{1,2})?\/[^\s'"`()<>[\]|;&]*[^\s'"`()<>[\]|;&.,:]|[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.@-]+)+\.[A-Za-z0-9]+)(?::(\d+))?(?::\d+)?/g;

/** Finds file paths in one line of terminal output. */
export function findPathLinks(text: string): PathLink[] {
  const links: PathLink[] = [];
  for (const match of text.matchAll(pathPattern)) {
    const path = match[1];
    if (path === "/" || /^\/\//.test(path) || /^[a-z]+:\/\//i.test(path))
      continue;
    const start = (match.index ?? 0) + match[0].indexOf(path);
    const end = start + match[0].length - match[0].indexOf(path);
    links.push({
      start,
      end,
      path,
      line: match[2] ? Number(match[2]) : undefined,
    });
  }
  return links.filter(
    (link) => !/https?:\/\/\S*$/.test(text.slice(0, link.start + 1)),
  );
}

/** Makes a path absolute using the terminal's current folder and home. */
export function resolvePath(path: string, cwd: string, home: string): string {
  const base = path.startsWith("~/")
    ? `${home}/${path.slice(2)}`
    : path.startsWith("/")
      ? path
      : `${cwd || home}/${path}`;
  const parts: string[] = [];
  for (const part of base.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return `/${parts.join("/")}`;
}

export function parentFolder(path: string): string {
  const index = path.replace(/\/+$/, "").lastIndexOf("/");
  return index > 0 ? path.slice(0, index) : "/";
}
