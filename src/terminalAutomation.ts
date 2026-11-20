/**
 * Expect-style automation for a live terminal. A plan is a list of lines:
 *
 *   expect login:          wait for text (case-insensitive)
 *   expect /[$#] $/        wait for a regular expression
 *   send admin\n           type text; \n is Enter, \t is Tab
 *   secret                 ask for a value when the run starts and type it
 *   wait 2                 pause for seconds
 *   timeout 60             seconds each later expect may take (default 30)
 *   # comment
 */

export type AutomationStep =
  | { kind: "expect"; pattern: RegExp; label: string; line: number }
  | { kind: "send"; text: string; line: number }
  | { kind: "secret"; prompt: string; line: number }
  | { kind: "wait"; seconds: number; line: number }
  | { kind: "timeout"; seconds: number; line: number };

const escapeRegex = (text: string) =>
  text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export function decodeSendText(text: string): string {
  return text.replace(/\\([nrt\\])/g, (_, code: string) =>
    code === "n" || code === "r" ? "\r" : code === "t" ? "\t" : "\\",
  );
}

/** Parses a plan, throwing a message that names the line to fix. */
export function parseAutomation(source: string): AutomationStep[] {
  const steps: AutomationStep[] = [];
  source.split("\n").forEach((raw, index) => {
    const line = index + 1;
    const text = raw.trim();
    if (!text || text.startsWith("#")) return;
    const [command, ...rest] = text.split(/\s+/);
    const argument = raw.trimStart().slice(command.length).replace(/^ /, "");
    switch (command.toLowerCase()) {
      case "expect": {
        if (!argument.trim())
          throw new Error(`Line ${line}: write the text to wait for.`);
        const regex = /^\/(.+)\/$/.exec(argument.trim());
        let pattern: RegExp;
        try {
          pattern = regex
            ? new RegExp(regex[1], "i")
            : new RegExp(escapeRegex(argument.trim()), "i");
        } catch {
          throw new Error(
            `Line ${line}: the pattern is not a valid regular expression.`,
          );
        }
        steps.push({ kind: "expect", pattern, label: argument.trim(), line });
        break;
      }
      case "send":
        if (!argument) throw new Error(`Line ${line}: write the text to type.`);
        steps.push({ kind: "send", text: decodeSendText(argument), line });
        break;
      case "secret":
        steps.push({
          kind: "secret",
          prompt: argument.trim() || `Value for line ${line}`,
          line,
        });
        break;
      case "wait":
      case "timeout": {
        const seconds = Number(rest[0]);
        if (!Number.isFinite(seconds) || seconds <= 0 || seconds > 3600)
          throw new Error(
            `Line ${line}: ${command} needs a number of seconds between 0 and 3600.`,
          );
        steps.push({ kind: command.toLowerCase() as "wait", seconds, line });
        break;
      }
      default:
        throw new Error(
          `Line ${line}: “${command}” is not a step. Use expect, send, secret, wait or timeout.`,
        );
    }
  });
  if (!steps.length) throw new Error("Write at least one step.");
  return steps;
}

const controlSequence =
  /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\].*?(?:\x07|\x1b\\)|[\x00-\x08\x0b-\x0c\x0e-\x1f\x7f]/g;

export type AutomationStatus =
  | { state: "running"; step: number; waitingFor?: string }
  | { state: "done" }
  | { state: "failed"; message: string };

type Host = {
  write: (data: string) => void;
  /** Asks for a secret value; resolves to null when the user cancels. */
  askSecret: (prompt: string) => Promise<string | null>;
  onStatus: (status: AutomationStatus) => void;
  now?: () => number;
  setTimer?: (callback: () => void, ms: number) => unknown;
  clearTimer?: (timer: unknown) => void;
};

/** Runs a parsed plan against live terminal output. */
export class AutomationRun {
  private index = 0;
  private buffer = "";
  private timeoutMs = 30_000;
  private timer: unknown;
  private finished = false;
  private readonly setTimer: NonNullable<Host["setTimer"]>;
  private readonly clearTimer: NonNullable<Host["clearTimer"]>;

  constructor(
    private steps: AutomationStep[],
    private host: Host,
  ) {
    this.setTimer = host.setTimer ?? ((cb, ms) => window.setTimeout(cb, ms));
    this.clearTimer =
      host.clearTimer ?? ((timer) => window.clearTimeout(timer as number));
  }

  get active() {
    return !this.finished;
  }

  start() {
    void this.advance();
  }

  stop(message = "Stopped.") {
    if (this.finished) return;
    this.finished = true;
    this.clearTimer(this.timer);
    this.host.onStatus({ state: "failed", message });
  }

  /** Feeds terminal output; matching continues across chunks. */
  output(text: string) {
    if (this.finished) return;
    this.buffer = (this.buffer + text.replace(controlSequence, "")).slice(
      -16_384,
    );
    const step = this.steps[this.index];
    if (step?.kind !== "expect") return;
    const match = step.pattern.exec(this.buffer);
    if (!match) return;
    this.buffer = this.buffer.slice(match.index + match[0].length);
    this.clearTimer(this.timer);
    this.index++;
    void this.advance();
  }

  private async advance() {
    while (!this.finished && this.index < this.steps.length) {
      const step = this.steps[this.index];
      switch (step.kind) {
        case "expect":
          this.host.onStatus({
            state: "running",
            step: this.index + 1,
            waitingFor: step.label,
          });
          this.timer = this.setTimer(
            () =>
              this.stop(
                `Line ${step.line}: “${step.label}” did not appear within ${this.timeoutMs / 1000} seconds.`,
              ),
            this.timeoutMs,
          );
          // Text that already arrived may satisfy the step.
          this.output("");
          return;
        case "send":
          this.host.write(step.text);
          break;
        case "secret": {
          const value = await this.host.askSecret(step.prompt);
          if (value === null) {
            this.stop("Cancelled.");
            return;
          }
          this.host.write(value + "\r");
          break;
        }
        case "wait":
          this.host.onStatus({ state: "running", step: this.index + 1 });
          await new Promise((resolve) =>
            this.setTimer(() => resolve(undefined), step.seconds * 1000),
          );
          break;
        case "timeout":
          this.timeoutMs = step.seconds * 1000;
          break;
      }
      this.index++;
    }
    if (!this.finished) {
      this.finished = true;
      this.host.onStatus({ state: "done" });
    }
  }
}

export type SavedAutomation = { id: string; name: string; body: string };
const storageKey = "weave-terminal-automations";

export function readAutomations(
  storage: Pick<Storage, "getItem">,
): SavedAutomation[] {
  try {
    const parsed: unknown = JSON.parse(storage.getItem(storageKey) ?? "[]");
    return Array.isArray(parsed)
      ? parsed.filter(
          (item): item is SavedAutomation =>
            typeof item?.id === "string" &&
            typeof item?.name === "string" &&
            typeof item?.body === "string",
        )
      : [];
  } catch {
    return [];
  }
}

export function writeAutomations(
  storage: Pick<Storage, "setItem">,
  automations: SavedAutomation[],
) {
  if (automations.length > 30) throw new Error("Save up to 30 automations.");
  storage.setItem(storageKey, JSON.stringify(automations));
}
