import { createSignal, For, Show } from "solid-js";
import { X } from "lucide-solid";
import { revealInTerminal, searchTerminals } from "./terminalRegistry";
import type { LineMatch } from "./terminalPaste";

type Props = {
  sessions: { key: string; label: string }[];
  onShow: (key: string) => void;
  onClose: () => void;
};

/** Searches the scrollback of every open terminal tab at once. */
export default function TerminalSearchAll(props: Props) {
  const [query, setQuery] = createSignal("");
  const [results, setResults] = createSignal<
    { key: string; matches: LineMatch[] }[]
  >([]);
  const [searched, setSearched] = createSignal(false);
  const label = (key: string) =>
    props.sessions.find((session) => session.key === key)?.label ?? "Terminal";

  function run(event: Event) {
    event.preventDefault();
    setResults(
      searchTerminals(query().trim()).filter((tab) => tab.matches.length),
    );
    setSearched(true);
  }

  return (
    <div class="overlay" onClick={props.onClose}>
      <div
        class="dialog profile-dialog terminal-search-all"
        role="dialog"
        aria-modal="true"
        aria-label="Find in all terminal tabs"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="dialog-head">
          <div>
            <span class="eyebrow">TERMINAL</span>
            <h2>Find in all tabs</h2>
          </div>
          <button
            type="button"
            class="icon-button"
            aria-label="Close"
            onClick={props.onClose}
          >
            <X size={19} />
          </button>
        </div>
        <form class="ai-key-form" onSubmit={run}>
          <input
            ref={(element) => queueMicrotask(() => element.focus())}
            aria-label="Text to find"
            placeholder="Text in any terminal's output"
            value={query()}
            onInput={(event) => setQuery(event.currentTarget.value)}
          />
          <button
            class="primary-button"
            type="submit"
            disabled={!query().trim()}
          >
            Find
          </button>
        </form>
        <div class="terminal-search-results">
          <Show when={searched() && !results().length}>
            <p class="muted">No open terminal shows that text.</p>
          </Show>
          <For each={results()}>
            {(tab) => (
              <section>
                <h3>
                  {label(tab.key)} · {tab.matches.length}
                  {tab.matches.length > 200 ? "+" : ""} match
                  {tab.matches.length === 1 ? "" : "es"}
                </h3>
                <For each={tab.matches}>
                  {(match) => (
                    <button
                      type="button"
                      onClick={() => {
                        props.onShow(tab.key);
                        props.onClose();
                        requestAnimationFrame(() =>
                          revealInTerminal(tab.key, match.line),
                        );
                      }}
                    >
                      <span>{match.line + 1}</span>
                      <code>{match.text.trim() || " "}</code>
                    </button>
                  )}
                </For>
              </section>
            )}
          </For>
        </div>
      </div>
    </div>
  );
}
