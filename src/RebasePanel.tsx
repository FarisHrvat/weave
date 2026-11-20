import { confirmAction } from "./dialogs";
import { createEffect, createSignal, For, Show } from "solid-js";
import { api, errorMessage } from "./api";
import type { RebaseStep } from "./types";

type Props = {
  repository: string;
  onNotice: (message: string) => void;
  onChanged: () => void;
};

export default function RebasePanel(props: Props) {
  const [base, setBase] = createSignal("");
  const [loadedBase, setLoadedBase] = createSignal("");
  const [steps, setSteps] = createSignal<RebaseStep[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [mergePlan, setMergePlan] = createSignal("");
  let requestId = 0;

  createEffect(() => {
    props.repository;
    requestId += 1;
    setBase("");
    setLoadedBase("");
    setSteps([]);
    setBusy(false);
    setError("");
  });

  async function load(event: Event) {
    event.preventDefault();
    const currentRequest = ++requestId;
    const repository = props.repository;
    const requestedBase = base().trim();
    setBusy(true);
    setError("");
    try {
      const result = await api.gitRebasePlan(repository, requestedBase);
      if (currentRequest !== requestId || repository !== props.repository)
        return;
      setSteps(result);
      setLoadedBase(requestedBase);
    } catch (reason) {
      if (currentRequest !== requestId || repository !== props.repository)
        return;
      setSteps([]);
      setLoadedBase("");
      setError(errorMessage(reason));
    } finally {
      if (currentRequest === requestId) setBusy(false);
    }
  }

  function move(index: number, direction: -1 | 1) {
    const target = index + direction;
    const next = [...steps()];
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target], next[index]];
    setSteps(next);
  }

  function setAction(id: string, action: RebaseStep["action"]) {
    setSteps(
      steps().map((step) => (step.id === id ? { ...step, action } : step)),
    );
  }

  function setMessage(id: string, message: string) {
    setSteps(
      steps().map((step) => (step.id === id ? { ...step, message } : step)),
    );
  }

  async function rebaseKeepingMerges() {
    const onto = base().trim();
    if (!onto) {
      setError("Enter the branch or commit to rebase onto.");
      return;
    }
    if (
      !(await confirmAction(
        `Rebase the current branch onto ${onto}, recreating its merge commits? This rewrites the branch's history.`,
      ))
    )
      return;
    const currentRequest = ++requestId;
    const repository = props.repository;
    setBusy(true);
    setError("");
    try {
      const message = await api.gitRebaseMerges(repository, onto);
      if (currentRequest !== requestId || repository !== props.repository)
        return;
      setSteps([]);
      setLoadedBase("");
      props.onChanged();
      props.onNotice(message);
    } catch (reason) {
      if (currentRequest !== requestId || repository !== props.repository)
        return;
      setError(errorMessage(reason));
      props.onChanged();
    } finally {
      if (currentRequest === requestId) setBusy(false);
    }
  }

  async function loadMergePlan() {
    const onto = base().trim();
    if (!onto) {
      setError("Enter the branch or commit to rebase onto.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      setMergePlan(await api.gitRebaseMergesPlan(props.repository, onto));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  async function applyMergePlan() {
    if (!(await confirmAction("Rewrite the branch using this edited plan?")))
      return;
    setBusy(true);
    setError("");
    try {
      props.onNotice(
        await api.gitRebaseMergesApply(
          props.repository,
          base().trim(),
          mergePlan(),
        ),
      );
      setMergePlan("");
      props.onChanged();
    } catch (reason) {
      setError(errorMessage(reason));
      props.onChanged();
    } finally {
      setBusy(false);
    }
  }

  async function apply() {
    const dropped = steps().filter((step) => step.action === "drop").length;
    const detail = dropped
      ? `This will rewrite ${steps().length} commits and drop ${dropped}. Continue?`
      : `This will rewrite ${steps().length} commits. Continue?`;
    if (!(await confirmAction(detail))) return;
    const currentRequest = ++requestId;
    const repository = props.repository;
    setBusy(true);
    setError("");
    try {
      await api.gitRebaseApply(repository, loadedBase(), steps());
      if (currentRequest !== requestId || repository !== props.repository)
        return;
      setSteps([]);
      setLoadedBase("");
      props.onChanged();
      props.onNotice("Interactive rebase completed.");
    } catch (reason) {
      if (currentRequest !== requestId || repository !== props.repository)
        return;
      setError(errorMessage(reason));
      props.onChanged();
    } finally {
      if (currentRequest === requestId) setBusy(false);
    }
  }

  return (
    <div class="rebase-panel">
      <p class="muted">
        Reorder, reword, combine, or drop commits after a base commit on the
        current branch. Git must have a clean worktree. For branches with merge
        commits, use Rebase keeping merges to move the whole branch onto a new
        base.
      </p>
      <form class="commit-search" onSubmit={load}>
        <input
          aria-label="Interactive rebase base commit"
          value={base()}
          onInput={(event) => setBase(event.currentTarget.value)}
          placeholder="Base branch or commit, such as main or HEAD~3"
          required
        />
        <button class="mini-button" type="submit" disabled={busy()}>
          Load commits
        </button>
        <button
          class="mini-button"
          type="button"
          disabled={busy() || !base().trim()}
          title="git rebase --rebase-merges: moves the branch and recreates its merge commits"
          onClick={() => void rebaseKeepingMerges()}
        >
          Rebase keeping merges
        </button>
        <button
          class="mini-button"
          type="button"
          disabled={busy() || !base().trim()}
          title="Edit every step, including label, reset and merge lines"
          onClick={() => void loadMergePlan()}
        >
          Edit full plan
        </button>
      </form>
      <Show when={mergePlan()}>
        <div class="rebase-merge-plan">
          <p class="muted">
            Each line is a step. pick replays a commit, label and reset move
            between branches, merge recreates a merge, and drop or removing a
            line skips it.
          </p>
          <textarea
            aria-label="Merge-preserving rebase plan"
            spellcheck={false}
            value={mergePlan()}
            onInput={(event) => setMergePlan(event.currentTarget.value)}
          />
          <div class="rebase-actions">
            <button class="ghost-button" onClick={() => setMergePlan("")}>
              Cancel
            </button>
            <button
              class="primary-button"
              disabled={busy()}
              onClick={() => void applyMergePlan()}
            >
              Apply edited plan
            </button>
          </div>
        </div>
      </Show>
      <Show when={error()}>
        <p class="tailnet-error" role="alert">
          {error()}
        </p>
      </Show>
      <For each={steps()}>
        {(step, index) => (
          <>
            <div class="rebase-step">
              <span class="rebase-number">{index() + 1}</span>
              <div class="rebase-subject">
                <strong>{step.subject}</strong>
                <code>{step.id.slice(0, 8)}</code>
              </div>
              <select
                aria-label={`Action for ${step.subject}`}
                value={step.action}
                onChange={(event) =>
                  setAction(
                    step.id,
                    event.currentTarget.value as RebaseStep["action"],
                  )
                }
              >
                <option value="pick">Keep</option>
                <option value="reword">Reword</option>
                <option value="squash">Squash</option>
                <option value="fixup">Fixup</option>
                <option value="drop">Drop</option>
              </select>
              <button
                class="mini-button"
                type="button"
                aria-label={`Move ${step.subject} up`}
                disabled={busy() || index() === 0}
                onClick={() => move(index(), -1)}
              >
                ↑
              </button>
              <button
                class="mini-button"
                type="button"
                aria-label={`Move ${step.subject} down`}
                disabled={busy() || index() === steps().length - 1}
                onClick={() => move(index(), 1)}
              >
                ↓
              </button>
            </div>
            <Show when={step.action === "reword"}>
              <label class="rebase-message">
                Commit message
                <textarea
                  aria-label={`New commit message for ${step.subject}`}
                  value={step.message}
                  disabled={busy()}
                  rows={4}
                  onChange={(event) =>
                    setMessage(step.id, event.currentTarget.value)
                  }
                />
              </label>
            </Show>
          </>
        )}
      </For>
      <Show when={steps().length}>
        <div class="rebase-actions">
          <span class="muted">Base: {loadedBase()}</span>
          <button
            class="primary-button"
            disabled={busy()}
            onClick={() => void apply()}
          >
            Apply rebase
          </button>
        </div>
      </Show>
    </div>
  );
}
