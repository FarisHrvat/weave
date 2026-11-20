import { describe, expect, it } from "vitest";
import Zmodem from "zmodem.js";
import { ZmodemBridge, type ZmodemFile } from "./zmodemBridge";

const text = (bytes: Uint8Array) => new TextDecoder().decode(bytes);
const later = (job: () => void) => setTimeout(job, 0);

function bridge(options: {
  toRemote: (bytes: Uint8Array) => void;
  files?: ZmodemFile[] | null;
}) {
  const terminal: string[] = [];
  const saved: ZmodemFile[] = [];
  const status: string[] = [];
  const instance = new ZmodemBridge({
    toTerminal: (bytes) => terminal.push(text(bytes)),
    toRemote: options.toRemote,
    chooseFiles: async () => options.files ?? null,
    saveFile: async (file) => {
      saved.push(file);
      return `/Downloads/${file.name}`;
    },
    onStatus: (line) => status.push(line),
  });
  return { instance, terminal, saved, status };
}

async function until(check: () => boolean) {
  for (let tries = 0; tries < 400 && !check(); tries++)
    await new Promise((resolve) => setTimeout(resolve, 5));
  expect(check()).toBe(true);
}

describe("ZmodemBridge", () => {
  it("receives files the host sends with sz", async () => {
    let remote: ReturnType<typeof Zmodem.Session.parse>;
    const weave = bridge({
      toRemote: (bytes) =>
        later(() => {
          if (!remote) {
            remote = Zmodem.Session.parse(Array.from(bytes))!;
            remote.set_sender((octets) =>
              later(() => weave.instance.consume(Uint8Array.from(octets))),
            );
            void sendFiles();
          } else remote.consume(Array.from(bytes));
        }),
    });
    const content = new Uint8Array(20_000).map((_, index) => index % 251);
    async function sendFiles() {
      const transfer = await remote!.send_offer({
        name: "report.bin",
        size: content.length,
      });
      await transfer!.end(content);
      await remote!.close();
    }
    weave.instance.consume(new TextEncoder().encode("$ sz report.bin\r\n"));
    weave.instance.consume(
      Uint8Array.from(Zmodem.Header.build("ZRQINIT").to_hex()),
    );
    await until(() => weave.saved.length === 1);
    expect(weave.saved[0].name).toBe("report.bin");
    expect(weave.saved[0].bytes).toEqual(content);
    expect(weave.status.at(-1)).toBe("Saved /Downloads/report.bin");
    await until(() => !weave.instance.busy);
    weave.instance.consume(new TextEncoder().encode("$ "));
    expect(weave.terminal.join("")).toContain("$ sz report.bin");
    expect(weave.terminal.at(-1)).toBe("$ ");
  });

  it("sends chosen files when the host runs rz", async () => {
    const received: { name: string; bytes: Uint8Array }[] = [];
    const remote = Zmodem.Session.parse(
      Zmodem.Header.build("ZRQINIT").to_hex(),
    )!;
    const large = new Uint8Array(70_000).map((_, index) => (index * 7) % 256);
    const weave = bridge({
      toRemote: (bytes) => later(() => remote.consume(Array.from(bytes))),
      files: [
        { name: "notes.txt", bytes: new TextEncoder().encode("hello\n") },
        { name: "big.bin", bytes: large },
        { name: "empty.txt", bytes: new Uint8Array() },
      ],
    });
    remote.set_sender((octets) =>
      later(() => weave.instance.consume(Uint8Array.from(octets))),
    );
    remote.on("offer", (offer) => {
      const name = offer.get_details().name;
      void offer.accept().then((parts) => {
        const bytes = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
        let at = 0;
        for (const part of parts) {
          bytes.set(part, at);
          at += part.length;
        }
        received.push({ name, bytes });
      });
    });
    void remote.start();
    await until(() => weave.status.at(-1) === "Sent 3 files.");
    expect(received.map((file) => file.name)).toEqual([
      "notes.txt",
      "big.bin",
      "empty.txt",
    ]);
    expect(text(received[0].bytes)).toBe("hello\n");
    expect(received[1].bytes).toEqual(large);
    expect(received[2].bytes.length).toBe(0);
  });

  it("closes cleanly when no files are chosen", async () => {
    const remote = Zmodem.Session.parse(
      Zmodem.Header.build("ZRQINIT").to_hex(),
    )!;
    const weave = bridge({
      toRemote: (bytes) => later(() => remote.consume(Array.from(bytes))),
      files: null,
    });
    remote.set_sender((octets) =>
      later(() => weave.instance.consume(Uint8Array.from(octets))),
    );
    void remote.start();
    await until(() => weave.status.at(-1) === "Sending was cancelled.");
    await until(() => !weave.instance.busy);
  });

  it("passes ordinary output straight through", () => {
    const weave = bridge({ toRemote: () => undefined });
    weave.instance.consume(new TextEncoder().encode("ls -la\r\ntotal 0\r\n"));
    expect(weave.terminal.join("")).toBe("ls -la\r\ntotal 0\r\n");
  });
});
