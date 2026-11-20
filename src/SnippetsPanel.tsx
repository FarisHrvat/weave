import { createEffect, createSignal, For, onMount, Show } from "solid-js";
import { Plus, X } from "lucide-solid";
import {
  fillSnippet,
  readSnippets,
  snippetVariables,
  writeSnippets,
} from "./snippets";
import type { Snippet } from "./snippets";
import { errorMessage } from "./api";

type Props = {
  profileId?: string;
  onInsert: (text: string) => void;
  onClose: () => void;
  onError: (message: string) => void;
  onChange?: (snippets: Snippet[]) => void;
  initialSnippetId?: string;
  onRequestHandled?: () => void;
};

const blankSnippet = (profileId?: string): Snippet => ({
  id: crypto.randomUUID(),
  name: "",
  body: "",
  profileId: profileId ?? null,
});

export default function SnippetsPanel(props: Props) {
  const [snippets, setSnippets] = createSignal<Snippet[]>([]);
  const [editing, setEditing] = createSignal<Snippet>();
  const [pending, setPending] = createSignal<Snippet>();
  const [values, setValues] = createSignal<Record<string, string>>({});

  onMount(() => {
    try {
      const saved = readSnippets();
      setSnippets(saved);
    } catch (error) {
      props.onError(errorMessage(error));
    }
  });

  createEffect(() => {
    const id = props.initialSnippetId;
    if (!id) return;
    const selected = snippets().find((item) => item.id === id);
    if (!selected) return;
    insert(selected);
    props.onRequestHandled?.();
  });

  const visible = () =>
    snippets().filter(
      (item) => !item.profileId || item.profileId === props.profileId,
    );

  function save(event: Event) {
    event.preventDefault();
    const item = editing();
    if (!item) return;
    if (!item.name.trim() || !item.body.trim()) {
      props.onError("Give the snippet a name and a command.");
      return;
    }
    const next = [
      ...snippets().filter((saved) => saved.id !== item.id),
      { ...item, name: item.name.trim(), body: item.body.trim() },
    ];
    try {
      writeSnippets(next);
      setSnippets(next);
      props.onChange?.(next);
      setEditing(undefined);
    } catch (error) {
      props.onError(`Could not save snippet: ${errorMessage(error)}`);
    }
  }

  function remove(item: Snippet) {
    const next = snippets().filter((saved) => saved.id !== item.id);
    try {
      writeSnippets(next);
      setSnippets(next);
      props.onChange?.(next);
      setEditing(undefined);
    } catch (error) {
      props.onError(`Could not delete snippet: ${errorMessage(error)}`);
    }
  }

  function togglePinned(item: Snippet) {
    const next = snippets().map((saved) =>
      saved.id === item.id ? { ...saved, pinned: !saved.pinned } : saved,
    );
    try {
      writeSnippets(next);
      setSnippets(next);
      props.onChange?.(next);
    } catch (error) {
      props.onError(`Could not update command bar: ${errorMessage(error)}`);
    }
  }

  function insert(item: Snippet) {
    if (snippetVariables(item.body).length) {
      setValues({});
      setPending(item);
    } else props.onInsert(item.body);
  }

  function fill(event: Event) {
    event.preventDefault();
    const item = pending();
    if (!item) return;
    try {
      props.onInsert(fillSnippet(item.body, values()));
      setPending(undefined);
    } catch (error) {
      props.onError(errorMessage(error));
    }
  }

  return (
    <aside class="snippets-panel" aria-label="Terminal snippets">
      <div class="snippets-head">
        <div>
          <span class="eyebrow">TERMINAL</span>
          <h2>Snippets</h2>
        </div>
        <button
          class="icon-button"
          title="Close snippets"
          onClick={props.onClose}
        >
          <X size={18} />
        </button>
      </div>
      <p class="muted snippets-hint">
        Insert a saved command, then review it before pressing Enter.
      </p>
      <div class="snippets-list">
        <For
          each={visible()}
          fallback={
            <div class="empty-small">Save a command you use often.</div>
          }
        >
          {(item) => (
            <div class="snippet-card">
              <strong>{item.name}</strong>
              <code>{item.body}</code>
              <div>
                <button onClick={() => insert(item)}>Insert</button>
                <button onClick={() => togglePinned(item)}>
                  {item.pinned ? "Unpin" : "Pin to bar"}
                </button>
                <button onClick={() => setEditing({ ...item })}>Edit</button>
                <button class="danger-text" onClick={() => remove(item)}>
                  Delete
                </button>
              </div>
            </div>
          )}
        </For>
      </div>
      <button
        class="ghost-button snippet-add"
        onClick={() => setEditing(blankSnippet(props.profileId))}
      >
        <Plus size={15} /> New snippet
      </button>
      <Show when={editing()}>
        {(item) => (
          <div class="overlay" onClick={() => setEditing(undefined)}>
            <div
              class="dialog profile-dialog"
              role="dialog"
              aria-modal="true"
              aria-label="Edit snippet"
              onClick={(event) => event.stopPropagation()}
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">TERMINAL SNIPPET</span>
                  <h2>
                    {snippets().some((saved) => saved.id === item().id)
                      ? "Edit snippet"
                      : "New snippet"}
                  </h2>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close"
                  onClick={() => setEditing(undefined)}
                >
                  <X size={19} />
                </button>
              </div>
              <p class="muted">
                Use variables like <code>{"{{script}}"}</code> to fill values
                when you insert the command.
              </p>
              <form onSubmit={save}>
                <div class="form-grid">
                  <label class="wide">
                    Name
                    <input
                      required
                      value={item().name}
                      onInput={(event) =>
                        setEditing({
                          ...item(),
                          name: event.currentTarget.value,
                        })
                      }
                      placeholder="Submit batch job"
                    />
                  </label>
                  <label class="wide">
                    Command
                    <textarea
                      required
                      value={item().body}
                      onInput={(event) =>
                        setEditing({
                          ...item(),
                          body: event.currentTarget.value,
                        })
                      }
                      placeholder="sbatch {{script}}"
                    />
                  </label>
                  <label class="wide">
                    Available in
                    <select
                      value={item().profileId ?? ""}
                      onChange={(event) =>
                        setEditing({
                          ...item(),
                          profileId: event.currentTarget.value || null,
                        })
                      }
                    >
                      <option value="">All terminals</option>
                      <Show when={props.profileId}>
                        <option value={props.profileId}>This host</option>
                      </Show>
                    </select>
                  </label>
                </div>
                <div class="dialog-actions">
                  <span class="spacer" />
                  <button
                    type="button"
                    class="ghost-button"
                    onClick={() => setEditing(undefined)}
                  >
                    Cancel
                  </button>
                  <button type="submit" class="primary-button">
                    Save snippet
                  </button>
                </div>
              </form>
            </div>
          </div>
        )}
      </Show>
      <Show when={pending()}>
        {(item) => (
          <div class="overlay" onClick={() => setPending(undefined)}>
            <div
              class="dialog profile-dialog"
              role="dialog"
              aria-modal="true"
              aria-label="Snippet variables"
              onClick={(event) => event.stopPropagation()}
            >
              <div class="dialog-head">
                <div>
                  <span class="eyebrow">COMMAND VARIABLES</span>
                  <h2>{item().name}</h2>
                </div>
                <button
                  class="icon-button"
                  aria-label="Close"
                  onClick={() => setPending(undefined)}
                >
                  <X size={19} />
                </button>
              </div>
              <form onSubmit={fill}>
                <div class="form-grid">
                  <For each={snippetVariables(item().body)}>
                    {(name) => (
                      <label class="wide">
                        {name}
                        <input
                          required
                          value={values()[name] ?? ""}
                          onInput={(event) =>
                            setValues({
                              ...values(),
                              [name]: event.currentTarget.value,
                            })
                          }
                        />
                      </label>
                    )}
                  </For>
                </div>
                <div class="dialog-actions">
                  <span class="spacer" />
                  <button
                    type="button"
                    class="ghost-button"
                    onClick={() => setPending(undefined)}
                  >
                    Cancel
                  </button>
                  <button type="submit" class="primary-button">
                    Insert command
                  </button>
                </div>
              </form>
            </div>
          </div>
        )}
      </Show>
    </aside>
  );
}
