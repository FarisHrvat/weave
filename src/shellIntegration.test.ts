import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  installerScript,
  parseOsc133,
  uninstallerScript,
} from "./shellIntegration";

function run(script: string, home: string, shell: string) {
  return execFileSync("/bin/sh", ["-s"], {
    input: script,
    env: { HOME: home, SHELL: shell, PATH: "/usr/bin:/bin" },
  }).toString();
}

describe("shell integration", () => {
  it("installs once, keeps existing startup lines, and uninstalls cleanly", () => {
    const home = mkdtempSync(join(tmpdir(), "weave-shell-"));
    writeFileSync(join(home, ".zshrc"), "export KEEP=1\n");
    expect(run(installerScript(), home, "/bin/zsh")).toContain(".zshrc");
    expect(run(installerScript(), home, "/bin/zsh")).toContain("already");
    const rc = readFileSync(join(home, ".zshrc"), "utf8");
    expect(rc.match(/weave shell integration >>>/g)).toHaveLength(1);
    expect(rc).toContain("export KEEP=1");
    expect(existsSync(join(home, ".bashrc"))).toBe(false);
    const zsh = join(home, ".config/weave/weave.zsh");
    const bash = join(home, ".config/weave/weave.bash");
    execFileSync("/bin/zsh", ["-n", zsh]);
    execFileSync("/bin/bash", ["-n", bash]);

    run(uninstallerScript(), home, "/bin/zsh");
    expect(readFileSync(join(home, ".zshrc"), "utf8")).toBe("export KEEP=1\n");
    expect(existsSync(zsh)).toBe(false);
  });

  it("reports prompt marks from an interactive zsh", () => {
    const home = mkdtempSync(join(tmpdir(), "weave-shell-"));
    run(installerScript(), home, "/bin/zsh");
    const output = execFileSync("/bin/zsh", ["-i"], {
      input: "cd /tmp\nfalse\nexit 0\n",
      env: {
        HOME: home,
        SHELL: "/bin/zsh",
        PATH: "/usr/bin:/bin",
        TERM: "dumb",
      },
    }).toString();
    expect(output).toContain("\x1b]133;A\x07");
    expect(output).toMatch(/\x1b\]7;file:\/\/[^\x07]*\/(private\/)?tmp\x07/);
    expect(output).toContain("\x1b]133;D;1\x07");
  });

  it("installs and removes the fish integration when fish is set up", () => {
    const home = mkdtempSync(join(tmpdir(), "weave-shell-"));
    execFileSync("/bin/mkdir", ["-p", join(home, ".config/fish")]);
    run(installerScript(), home, "/bin/zsh");
    const fish = join(home, ".config/fish/conf.d/weave.fish");
    const script = readFileSync(fish, "utf8");
    expect(script).toContain("printf '\\e]133;A\\a'");
    expect(script).toContain("--on-event fish_postexec");
    run(uninstallerScript(), home, "/bin/zsh");
    expect(existsSync(fish)).toBe(false);
  });

  it("parses OSC 133 marks", () => {
    expect(parseOsc133("A")).toEqual({ kind: "prompt" });
    expect(parseOsc133("D;2")).toEqual({ kind: "finished", exitCode: 2 });
    expect(parseOsc133("D")).toEqual({ kind: "finished", exitCode: null });
    expect(parseOsc133("Z")).toBeNull();
  });
});
