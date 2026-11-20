// Minimal types for the Node built-ins used by tests that run real shells.
// The app itself never imports these modules.
declare module "node:child_process" {
  export function execFileSync(
    file: string,
    args?: string[],
    options?: { input?: string; env?: Record<string, string> },
  ): { toString(): string };
}
declare module "node:fs" {
  export function mkdtempSync(prefix: string): string;
  export function readFileSync(path: string, encoding: "utf8"): string;
  export function writeFileSync(path: string, data: string): void;
  export function existsSync(path: string): boolean;
}
declare module "node:os" {
  export function tmpdir(): string;
}
declare module "node:path" {
  export function join(...parts: string[]): string;
}
