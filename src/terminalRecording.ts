type RecordingEvent = [number, "o" | "r", string];

export class TerminalRecording {
  private readonly start: number;
  private readonly timestamp: number;
  private readonly width: number;
  private readonly height: number;
  private readonly decoder = new TextDecoder();
  private readonly events: RecordingEvent[] = [];
  private bytes = 0;
  private stopped = false;
  private currentWidth: number;
  private currentHeight: number;

  constructor(
    width: number,
    height: number,
    now = performance.now(),
    timestamp = Date.now(),
  ) {
    this.width = width;
    this.height = height;
    this.currentWidth = width;
    this.currentHeight = height;
    this.start = now;
    this.timestamp = Math.floor(timestamp / 1000);
  }

  output(chunk: Uint8Array, now = performance.now()): boolean {
    if (this.stopped) return false;
    this.bytes += chunk.byteLength;
    if (this.bytes > 4 * 1024 * 1024) {
      this.stopped = true;
      return false;
    }
    const text = this.decoder.decode(chunk, { stream: true });
    if (text) this.events.push([this.elapsed(now), "o", text]);
    return true;
  }

  resize(width: number, height: number, now = performance.now()) {
    if (
      this.stopped ||
      (width === this.currentWidth && height === this.currentHeight)
    )
      return;
    this.currentWidth = width;
    this.currentHeight = height;
    this.events.push([this.elapsed(now), "r", `${width}x${height}`]);
  }

  finish(now = performance.now()): string {
    const tail = this.decoder.decode();
    if (tail) this.events.push([this.elapsed(now), "o", tail]);
    this.stopped = true;
    const header = {
      version: 2,
      width: this.width,
      height: this.height,
      timestamp: this.timestamp,
      env: { TERM: "xterm-256color" },
    };
    return (
      [
        JSON.stringify(header),
        ...this.events.map((event) => JSON.stringify(event)),
      ].join("\n") + "\n"
    );
  }

  plainText(): string {
    const output = this.events
      .filter((event) => event[1] === "o")
      .map((event) => event[2])
      .join("");
    let text = "";
    let state:
      | "text"
      | "escape"
      | "escapeIntermediate"
      | "csi"
      | "string"
      | "stringEscape" = "text";
    for (let index = 0; index < output.length; index++) {
      const character = output[index];
      if (state === "escape") {
        state =
          character === "["
            ? "csi"
            : "]P_^".includes(character)
              ? "string"
              : character >= " " && character <= "/"
                ? "escapeIntermediate"
                : "text";
      } else if (state === "escapeIntermediate") {
        if (character >= "0" && character <= "~") state = "text";
      } else if (state === "csi") {
        if (character >= "@" && character <= "~") state = "text";
      } else if (state === "string") {
        if (character === "\x07") state = "text";
        else if (character === "\x1b") state = "stringEscape";
      } else if (state === "stringEscape") {
        state = character === "\\" ? "text" : "string";
      } else if (character === "\x1b") {
        state = "escape";
      } else if (character === "\r") {
        text += "\n";
        if (output[index + 1] === "\n") index++;
      } else if (character === "\b") {
        if (text.length && !text.endsWith("\n")) text = text.slice(0, -1);
      } else if (character === "\n" || character === "\t" || character >= " ") {
        text += character;
      }
    }
    return text;
  }

  private elapsed(now: number): number {
    return Math.max(0, Math.round((now - this.start) * 1000) / 1_000_000);
  }
}
