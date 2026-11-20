export type TerminalTriggerAction =
  "alert" | "notify" | "respond" | "highlight";

/**
 * A rule that watches terminal output. Older saved rules only have `id` and
 * `phrase`; they behave as plain-text in-app alerts.
 */
export type TerminalTrigger = {
  id: string;
  phrase: string;
  regex?: boolean;
  action?: TerminalTriggerAction;
  /** Text typed into the terminal for the "respond" action. */
  response?: string;
};

export const triggerActionLabels: Record<TerminalTriggerAction, string> = {
  alert: "Show alert",
  notify: "System notification",
  respond: "Type a response",
  highlight: "Highlight line",
};

const actions = Object.keys(triggerActionLabels);
export const maxTriggers = 24;

/** Returns an error message for an invalid rule, or an empty string. */
export function validateTrigger(trigger: Omit<TerminalTrigger, "id">): string {
  if (!trigger.phrase || trigger.phrase.length > 80)
    return "Enter a trigger phrase of 1 to 80 characters.";
  if (trigger.regex) {
    try {
      new RegExp(trigger.phrase, "i");
    } catch {
      return "The trigger pattern is not a valid regular expression.";
    }
    if (new RegExp(trigger.phrase, "i").test(""))
      return "The trigger pattern must not match empty output.";
  }
  if (trigger.action !== undefined && !actions.includes(trigger.action))
    return "Choose a trigger action.";
  if (trigger.action === "respond") {
    if (!trigger.response || trigger.response.length > 200)
      return "Enter a response of 1 to 200 characters.";
  }
  return "";
}

/** Converts a response with \n, \r and \t escapes into the text to type. */
export function decodeTriggerResponse(response: string): string {
  return response.replace(/\\([nrt\\])/g, (_, code: string) =>
    code === "n" ? "\r" : code === "r" ? "\r" : code === "t" ? "\t" : "\\",
  );
}

const storageKey = "weave-terminal-triggers";

export function readTerminalTriggers(
  storage: Pick<Storage, "getItem">,
): TerminalTrigger[] {
  const saved = storage.getItem(storageKey);
  if (!saved) return [];
  let value: unknown;
  try {
    value = JSON.parse(saved);
  } catch {
    throw new Error(
      "Saved terminal triggers are damaged. Remove the weave-terminal-triggers entry from this app's storage to reset them.",
    );
  }
  if (
    !Array.isArray(value) ||
    value.length > maxTriggers ||
    !value.every(
      (item) =>
        typeof item.id === "string" &&
        typeof item.phrase === "string" &&
        (item.regex === undefined || typeof item.regex === "boolean") &&
        (item.response === undefined || typeof item.response === "string") &&
        validateTrigger(item) === "",
    )
  ) {
    throw new Error(
      "Saved terminal triggers have an invalid format. Remove the weave-terminal-triggers entry from this app's storage to reset them.",
    );
  }
  return value as TerminalTrigger[];
}

export function writeTerminalTriggers(
  storage: Pick<Storage, "setItem">,
  triggers: TerminalTrigger[],
): void {
  storage.setItem(storageKey, JSON.stringify(triggers));
}

const controlSequence =
  /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\].*?(?:\x07|\x1b\\)|[\x00-\x08\x0b-\x1f\x7f]/g;

export type TriggerMatch = { trigger: TerminalTrigger; line: string };

function matches(trigger: TerminalTrigger, visible: string): boolean {
  if (trigger.regex) {
    try {
      return new RegExp(trigger.phrase, "i").test(visible);
    } catch {
      return false;
    }
  }
  return visible
    .toLocaleLowerCase()
    .includes(trigger.phrase.toLocaleLowerCase());
}

export class TerminalTriggerMatcher {
  private decoder = new TextDecoder();
  private pending = "";
  private lastResponse = new Map<string, number>();

  /** Returns the phrases of matching rules, in output order. */
  feed(bytes: Uint8Array, triggers: TerminalTrigger[]): string[] {
    return this.feedMatches(bytes, triggers).map(
      (match) => match.trigger.phrase,
    );
  }

  /**
   * Returns matching rules with the visible line that matched. Prompts that
   * wait for input (such as "Password:") are checked without a line ending,
   * but only for rules that type a response, and only once per prompt.
   */
  feedMatches(
    bytes: Uint8Array,
    triggers: TerminalTrigger[],
    now = Date.now(),
  ): TriggerMatch[] {
    this.pending += this.decoder.decode(bytes, { stream: true });
    const lines = this.pending.split(/[\r\n]/);
    this.pending = lines.pop()?.slice(-4096) ?? "";
    const matched: TriggerMatch[] = [];
    for (const line of lines) {
      const visible = line.replace(controlSequence, "");
      for (const trigger of triggers) {
        if (matches(trigger, visible)) matched.push({ trigger, line: visible });
      }
    }
    const prompt = this.pending.replace(controlSequence, "");
    if (prompt) {
      for (const trigger of triggers) {
        if (trigger.action !== "respond" || !matches(trigger, prompt)) continue;
        // A response is typed at most once every 2 seconds per rule, so a
        // prompt that repeats its own text cannot cause a runaway loop.
        if (now - (this.lastResponse.get(trigger.id) ?? -Infinity) < 2000)
          continue;
        this.lastResponse.set(trigger.id, now);
        matched.push({ trigger, line: prompt });
        this.pending = "";
        break;
      }
    }
    return matched.filter((match) => {
      if (match.trigger.action !== "respond" || match.line === prompt)
        return true;
      const last = this.lastResponse.get(match.trigger.id) ?? -Infinity;
      if (now - last < 2000) return false;
      this.lastResponse.set(match.trigger.id, now);
      return true;
    });
  }
}
