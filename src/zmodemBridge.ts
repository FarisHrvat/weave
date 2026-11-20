import Zmodem from "zmodem.js";

export type ZmodemFile = { name: string; bytes: Uint8Array; mtime?: Date };

export type ZmodemHost = {
  toTerminal: (bytes: Uint8Array) => void;
  toRemote: (bytes: Uint8Array) => void;
  chooseFiles: () => Promise<ZmodemFile[] | null>;
  saveFile: (file: ZmodemFile) => Promise<string>;
  onStatus: (text: string, error?: boolean) => void;
};

/** Files bigger than this are skipped; they would have to fit in memory. */
export const maxZmodemBytes = 512 * 1024 * 1024;
const chunkSize = 8 * 1024;

function joined(parts: Uint8Array[]) {
  const all = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    all.set(part, offset);
    offset += part.length;
  }
  return all;
}

/**
 * Watches terminal output for `sz` and `rz` (ZMODEM) and carries out the
 * transfer: received files are saved, and `rz` asks which files to send.
 */
export class ZmodemBridge {
  private sentry: InstanceType<typeof Zmodem.Sentry>;
  private active = false;

  constructor(private host: ZmodemHost) {
    this.sentry = new Zmodem.Sentry({
      to_terminal: (octets) => {
        if (octets.length) host.toTerminal(Uint8Array.from(octets));
      },
      sender: (octets) => host.toRemote(Uint8Array.from(octets)),
      on_retract: () => undefined,
      on_detect: (detection) => {
        if (this.active) {
          detection.deny();
          return;
        }
        const session = detection.confirm();
        this.active = true;
        session.on("session_end", () => {
          this.active = false;
        });
        if (session.type === "receive") this.receive(session);
        else void this.send(session);
      },
    });
  }

  get busy() {
    return this.active;
  }

  consume(bytes: Uint8Array) {
    try {
      this.sentry.consume(Array.from(bytes));
    } catch (error) {
      this.active = false;
      this.host.onStatus(`The ZMODEM transfer stopped: ${String(error)}`, true);
    }
  }

  private receive(session: ReturnType<ZmodemSentryDetection["confirm"]>) {
    this.host.onStatus("Receiving files…");
    const saved: string[] = [];
    session.on("offer", (offer) => {
      const details = offer.get_details();
      if ((details.size ?? 0) > maxZmodemBytes) {
        this.host.onStatus(
          `Skipped ${details.name}: files over 512 MB need SFTP instead.`,
          true,
        );
        offer.skip();
        return;
      }
      offer
        .accept()
        .then((parts) =>
          this.host.saveFile({
            name: details.name,
            bytes: joined(parts),
            mtime: details.mtime,
          }),
        )
        .then((path) => {
          saved.push(path);
          this.host.onStatus(
            saved.length === 1
              ? `Saved ${path}`
              : `Saved ${saved.length} files, the last as ${path}`,
          );
        })
        .catch((error) =>
          this.host.onStatus(
            `Could not save ${details.name}: ${String(error)}`,
            true,
          ),
        );
    });
    void session.start();
  }

  private async send(session: ReturnType<ZmodemSentryDetection["confirm"]>) {
    try {
      const files = (await this.host.chooseFiles()) ?? [];
      let remaining = files.reduce((sum, file) => sum + file.bytes.length, 0);
      for (const [index, file] of files.entries()) {
        this.host.onStatus(`Sending ${file.name}…`);
        const transfer = await session.send_offer({
          name: file.name,
          size: file.bytes.length,
          mtime: file.mtime,
          files_remaining: files.length - index,
          bytes_remaining: remaining,
        });
        remaining -= file.bytes.length;
        if (!transfer) {
          this.host.onStatus(`The host skipped ${file.name}.`);
          continue;
        }
        let at = 0;
        for (; file.bytes.length - at > chunkSize; at += chunkSize)
          transfer.send(file.bytes.subarray(at, at + chunkSize));
        await transfer.end(file.bytes.subarray(at));
      }
      await session.close();
      this.host.onStatus(
        files.length
          ? `Sent ${files.length} file${files.length === 1 ? "" : "s"}.`
          : "Sending was cancelled.",
      );
    } catch (error) {
      try {
        session.abort();
      } catch {
        // Already ended.
      }
      this.active = false;
      this.host.onStatus(`The ZMODEM upload stopped: ${String(error)}`, true);
    }
  }
}

type ZmodemSentryDetection = Parameters<
  ConstructorParameters<typeof Zmodem.Sentry>[0]["on_detect"]
>[0];
