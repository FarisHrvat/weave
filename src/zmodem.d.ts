declare module "zmodem.js" {
  type Octets = number[];
  type Details = { name: string; size?: number; mtime?: Date };
  interface Transfer {
    send(bytes: Octets | Uint8Array): void;
    end(bytes?: Octets | Uint8Array): Promise<void>;
  }
  interface Offer {
    get_details(): Details;
    accept(): Promise<Uint8Array[]>;
    skip(): void;
  }
  interface Session {
    type: "send" | "receive";
    on(event: "offer", handler: (offer: Offer) => void): Session;
    on(event: "session_end", handler: () => void): Session;
    start(): Promise<void>;
    send_offer(details: {
      name: string;
      size: number;
      mtime?: Date;
      files_remaining?: number;
      bytes_remaining?: number;
    }): Promise<Transfer | undefined>;
    close(): Promise<void>;
    abort(): void;
    consume(octets: Octets): void;
    set_sender(sender: (octets: Octets) => void): Session;
    has_ended(): boolean;
  }
  interface Detection {
    confirm(): Session;
    deny(): void;
    get_session_role(): "send" | "receive";
  }
  class Sentry {
    constructor(options: {
      to_terminal: (octets: Octets) => void;
      sender: (octets: Octets) => void;
      on_detect: (detection: Detection) => void;
      on_retract: () => void;
    });
    consume(octets: Octets | ArrayBuffer): void;
  }
  const Zmodem: {
    Sentry: typeof Sentry;
    Session: { parse(octets: Octets): Session | undefined };
    Header: { build(name: string): { to_hex(): Octets } };
  };
  export default Zmodem;
}
