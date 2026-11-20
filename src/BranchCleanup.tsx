import { confirmAction } from "./dialogs";
import { createSignal, For, Show } from "solid-js";
import { X } from "lucide-solid";
import { api, errorMessage } from "./api";
import type { BranchDeleteReport } from "./types";

type Props = {
  path: string;
  branches: string[];
  current: string;
  onClose: () => void;
  onDeleted: (report: BranchDeleteReport) => void;
  onError: (message: string) => void;
};

/** Deletes several local branches at once; unmerged ones are kept unless forced. */
export default function BranchCleanup(props: Props) {
  const [chosen, setChosen] = createSignal<string[]>([]);
  const [force, setForce] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [report, setReport] = createSignal<BranchDeleteReport>();
  const choices = () => props.branches.filter((name) => name !== props.current);

  function toggle(name: string, on: boolean) {
    setChosen((names) =>
      on ? [...names, name] : names.filter((item) => item !== name),
    );
  }

  async function remove(event: Event) {
    event.preventDefault();
    if (
      force() &&
      !(await confirmAction(
        `Delete ${chosen().length} branch${chosen().length === 1 ? "" : "es"} even if their commits are not merged? Weave keeps a record so you can restore them.`,
      ))
    )
      return;
    setBusy(true);
    try {
      const result = await api.gitBranchesDelete(props.path, chosen(), force());
      setReport(result);
      setChosen([]);
      props.onDeleted(result);
    } catch (error) {
      props.onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="overlay" onClick={props.onClose}>
      <form
        class="dialog profile-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Delete branches"
        onClick={(event) => event.stopPropagation()}
        onSubmit={remove}
      >
        <div class="dialog-head">
          <div>
            <span class="eyebrow">BRANCHES</span>
            <h2>Delete several branches</h2>
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
        <div class="branch-cleanup-list">
          <For
            each={choices()}
            fallback={<p class="muted">There are no other local branches.</p>}
          >
            {(name) => (
              <label class="check-label">
                <input
                  type="checkbox"
                  checked={chosen().includes(name)}
                  onChange={(event) =>
                    toggle(name, event.currentTarget.checked)
                  }
                />
                <span>{name}</span>
              </label>
            )}
          </For>
        </div>
        <label class="check-label">
          <input
            type="checkbox"
            checked={force()}
            onChange={(event) => setForce(event.currentTarget.checked)}
          />
          <span>Also delete branches whose commits are not merged</span>
        </label>
        <Show when={report()}>
          {(result) => (
            <div class="branch-cleanup-report" role="status">
              <Show when={result().deleted.length}>
                <p>Deleted: {result().deleted.join(", ")}</p>
              </Show>
              <For each={result().kept}>
                {([name, reason]) => (
                  <p class="form-error">
                    Kept {name}: {reason}
                  </p>
                )}
              </For>
            </div>
          )}
        </Show>
        <div class="dialog-actions">
          <button type="button" class="ghost-button" onClick={props.onClose}>
            Close
          </button>
          <button
            type="submit"
            class="ghost-button danger-text"
            disabled={busy() || chosen().length === 0}
          >
            {busy()
              ? "Deleting…"
              : `Delete ${chosen().length || ""} branch${chosen().length === 1 ? "" : "es"}`}
          </button>
        </div>
      </form>
    </div>
  );
}
