declare module "@novnc/novnc" {
  export default class RFB extends EventTarget {
    constructor(
      target: HTMLElement,
      url: string,
      options?: { credentials?: { username?: string; password?: string } },
    );
    scaleViewport: boolean;
    resizeSession: boolean;
    viewOnly: boolean;
    focusOnClick: boolean;
    background: string;
    sendCredentials(credentials: {
      username?: string;
      password?: string;
    }): void;
    sendCtrlAltDel(): void;
    clipboardPasteFrom(text: string): void;
    disconnect(): void;
    focus(): void;
  }
}
