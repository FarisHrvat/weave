/**
 * Shell integration for zsh, bash and fish. Once installed, the shell reports its
 * current folder (OSC 7) and marks prompts, commands and exit codes
 * (OSC 133, the FinalTerm convention also used by iTerm2 and VS Code).
 */

export const integrationVersion = 1;
const marker = "# >>> weave shell integration >>>";
const endMarker = "# <<< weave shell integration <<<";

export const zshIntegration = `# Weave shell integration v${integrationVersion}
[[ -o interactive ]] || return 0
[[ -n "$WEAVE_SHELL_INTEGRATION" ]] && return 0
export WEAVE_SHELL_INTEGRATION=${integrationVersion}
__weave_osc7() { printf '\\e]7;file://%s%s\\a' "\${HOST}" "\${PWD// /%20}"; }
__weave_precmd() {
  local code=$?
  if [[ -n "$__weave_ran" ]]; then printf '\\e]133;D;%s\\a' "$code"; fi
  unset __weave_ran
  __weave_osc7
  printf '\\e]133;A\\a'
}
__weave_preexec() { __weave_ran=1; printf '\\e]133;C\\a'; }
autoload -Uz add-zsh-hook
add-zsh-hook precmd __weave_precmd
add-zsh-hook preexec __weave_preexec
PS1="$PS1%{$(printf '\\e]133;B\\a')%}"
`;

export const bashIntegration = `# Weave shell integration v${integrationVersion}
[[ $- == *i* ]] || return 0
[[ -n "$WEAVE_SHELL_INTEGRATION" ]] && return 0
export WEAVE_SHELL_INTEGRATION=${integrationVersion}
__weave_osc7() { printf '\\e]7;file://%s%s\\a' "\${HOSTNAME}" "\${PWD// /%20}"; }
__weave_prompt() {
  local code=$?
  if [[ -n "$__weave_ran" ]]; then printf '\\e]133;D;%s\\a' "$code"; fi
  unset __weave_ran
  __weave_osc7
  printf '\\e]133;A\\a'
}
__weave_debug() {
  [[ "$BASH_COMMAND" == "__weave_prompt" || -n "$COMP_LINE" ]] && return
  [[ -z "$__weave_ran" ]] && { __weave_ran=1; printf '\\e]133;C\\a'; }
}
PROMPT_COMMAND="__weave_prompt\${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
trap '__weave_debug' DEBUG
PS1="$PS1\\[$(printf '\\e]133;B\\a')\\]"
`;

export const fishIntegration = `# Weave shell integration v${integrationVersion}
if status is-interactive; and not set -q WEAVE_SHELL_INTEGRATION
    set -gx WEAVE_SHELL_INTEGRATION ${integrationVersion}
    function __weave_prompt --on-event fish_prompt
        printf '\\e]7;file://%s%s\\a' (hostname) (string replace -a ' ' '%20' -- $PWD)
        printf '\\e]133;A\\a'
    end
    function __weave_preexec --on-event fish_preexec
        printf '\\e]133;C\\a'
    end
    function __weave_postexec --on-event fish_postexec
        printf '\\e]133;D;%s\\a' $status
    end
end
`;

function quoteHeredoc(body: string, tag: string) {
  return `cat > "$dir/weave.${tag}" <<'WEAVE_${tag.toUpperCase()}_EOF'\n${body}WEAVE_${tag.toUpperCase()}_EOF\n`;
}

/**
 * A POSIX sh script that installs the integration for the current user on
 * this Mac or on an SSH host. It writes the scripts to
 * ~/.config/weave and adds one guarded line to ~/.zshrc and ~/.bashrc when
 * those shells exist. Running it again updates the scripts without adding
 * duplicate lines.
 */
export function installerScript(): string {
  return `set -eu
dir="\${XDG_CONFIG_HOME:-$HOME/.config}/weave"
mkdir -p "$dir"
${quoteHeredoc(zshIntegration, "zsh")}${quoteHeredoc(bashIntegration, "bash")}
added=""
add_line() {
  rc="$1"; file="$2"
  [ -e "$rc" ] || [ "$3" = "create" ] || return 0
  if [ -f "$rc" ] && grep -qF '${marker}' "$rc"; then return 0; fi
  printf '\\n%s\\n[ -f "%s" ] && . "%s"\\n%s\\n' '${marker}' "$file" "$file" '${endMarker}' >> "$rc"
  added="$added $rc"
}
case "\${SHELL:-}" in
  */zsh) add_line "$HOME/.zshrc" "$dir/weave.zsh" create; add_line "$HOME/.bashrc" "$dir/weave.bash" existing ;;
  */bash) add_line "$HOME/.bashrc" "$dir/weave.bash" create; add_line "$HOME/.zshrc" "$dir/weave.zsh" existing ;;
  *) add_line "$HOME/.zshrc" "$dir/weave.zsh" existing; add_line "$HOME/.bashrc" "$dir/weave.bash" existing ;;
esac
fish_dir="\${XDG_CONFIG_HOME:-$HOME/.config}/fish"
if [ -d "$fish_dir" ] || command -v fish >/dev/null 2>&1; then
  mkdir -p "$fish_dir/conf.d"
  cat > "$fish_dir/conf.d/weave.fish" <<'WEAVE_FISH_EOF'
${fishIntegration}WEAVE_FISH_EOF
  added="$added $fish_dir/conf.d/weave.fish"
fi
echo "Installed Weave shell integration in $dir."
if [ -n "$added" ]; then echo "Added a source line to:$added"; else echo "Startup files were already set up."; fi
echo "Open a new terminal tab to start using it."
`;
}

/** Removes the guarded block from startup files and deletes the scripts. */
export function uninstallerScript(): string {
  return `set -eu
dir="\${XDG_CONFIG_HOME:-$HOME/.config}/weave"
for rc in "$HOME/.zshrc" "$HOME/.bashrc"; do
  [ -f "$rc" ] || continue
  grep -qF '${marker}' "$rc" || continue
  tmp="$rc.weave-tmp.$$"
  awk -v start='${marker}' -v end='${endMarker}' '
    $0 == start { skip = 1; blank = 0; next }
    $0 == end { skip = 0; next }
    skip { next }
    $0 == "" { if (blank) print ""; blank = 1; next }
    { if (blank) print ""; blank = 0; print }
    END { if (blank) print "" }
  ' "$rc" > "$tmp"
  cat "$tmp" > "$rc"
  rm -f "$tmp"
  echo "Removed Weave lines from $rc"
done
rm -f "$dir/weave.zsh" "$dir/weave.bash" "\${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/weave.fish"
echo "Weave shell integration was removed. Open a new tab to finish."
`;
}

export type PromptEvent =
  | { kind: "prompt" }
  | { kind: "input" }
  | { kind: "running" }
  | { kind: "finished"; exitCode: number | null };

/** Parses the data of an OSC 133 sequence. */
export function parseOsc133(data: string): PromptEvent | null {
  const [code, value] = data.split(";");
  switch (code) {
    case "A":
      return { kind: "prompt" };
    case "B":
      return { kind: "input" };
    case "C":
      return { kind: "running" };
    case "D": {
      const exit = value === undefined ? NaN : Number(value);
      return {
        kind: "finished",
        exitCode: Number.isInteger(exit) ? exit : null,
      };
    }
    default:
      return null;
  }
}
