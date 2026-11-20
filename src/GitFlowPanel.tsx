import { confirmAction } from "./dialogs";
import { createEffect, createSignal, Show } from "solid-js";
import { api, errorMessage } from "./api";
import type { GitFlowInfo } from "./types";

type Props = {
  repository: string;
  onNotice: (message: string) => void;
  onChanged: () => void;
};

type Settings = Omit<
  GitFlowInfo,
  "initialized" | "currentBranch" | "publishTags"
>;

const defaults: Settings = {
  mainBranch: "main",
  developBranch: "develop",
  featurePrefix: "feature/",
  releasePrefix: "release/",
  hotfixPrefix: "hotfix/",
  tagPrefix: "",
};

export default function GitFlowPanel(props: Props) {
  const [info, setInfo] = createSignal<GitFlowInfo>();
  const [settings, setSettings] = createSignal<Settings>(defaults);
  const [kind, setKind] = createSignal("feature");
  const [name, setName] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  let requestId = 0;

  createEffect(() => {
    const repository = props.repository;
    const currentRequest = ++requestId;
    setInfo(undefined);
    setSettings(defaults);
    setBusy(false);
    setError("");
    void api.gitFlowInfo(repository).then(
      (result) => {
        if (currentRequest !== requestId || repository !== props.repository)
          return;
        setInfo(result);
        setSettings({
          mainBranch: result.mainBranch,
          developBranch: result.developBranch,
          featurePrefix: result.featurePrefix,
          releasePrefix: result.releasePrefix,
          hotfixPrefix: result.hotfixPrefix,
          tagPrefix: result.tagPrefix,
        });
      },
      (reason) => {
        if (currentRequest === requestId) setError(errorMessage(reason));
      },
    );
  });

  function change(key: keyof Settings, value: string) {
    setSettings((previous) => ({ ...previous, [key]: value }));
  }

  async function act(
    operation: (repository: string) => Promise<GitFlowInfo>,
    notice: string,
  ) {
    const repository = props.repository;
    const currentRequest = ++requestId;
    setBusy(true);
    setError("");
    try {
      const result = await operation(repository);
      if (currentRequest !== requestId || repository !== props.repository)
        return;
      setInfo(result);
      setName("");
      props.onChanged();
      props.onNotice(notice);
    } catch (reason) {
      if (currentRequest !== requestId || repository !== props.repository)
        return;
      setError(errorMessage(reason));
      props.onChanged();
    } finally {
      if (currentRequest === requestId) setBusy(false);
    }
  }

  function saveSettings(event: Event) {
    event.preventDefault();
    const update = info()?.initialized ?? false;
    void act(
      (repository) => api.gitFlowSetup(repository, settings(), update),
      update
        ? "Gitflow settings saved."
        : "Gitflow set up for this repository.",
    );
  }

  function start(event: Event) {
    event.preventDefault();
    const branchType = kind();
    const branchName = name().trim();
    void act(
      (repository) => api.gitFlowStart(repository, branchType, branchName),
      `${branchType[0].toUpperCase()}${branchType.slice(1)} branch created.`,
    );
  }

  const activeKind = () => {
    const current = info()?.currentBranch ?? "";
    if (current.startsWith(info()?.featurePrefix ?? "\0")) return "feature";
    if (current.startsWith(info()?.releasePrefix ?? "\0")) return "release";
    if (current.startsWith(info()?.hotfixPrefix ?? "\0")) return "hotfix";
    return null;
  };

  async function finish() {
    const current = info();
    const branchType = activeKind();
    if (!current || !branchType) return;
    const detail =
      branchType === "feature"
        ? `Merge ${current.currentBranch} into ${current.developBranch} and delete the local branch?`
        : `Merge ${current.currentBranch} into ${current.mainBranch} and ${current.developBranch}, create a release tag, and delete the local branch? A conflict can leave the first merge completed.`;
    if (!(await confirmAction(detail))) return;
    void act(
      (repository) => api.gitFlowFinish(repository),
      `${branchType[0].toUpperCase()}${branchType.slice(1)} branch finished.`,
    );
  }

  async function publish() {
    const current = info();
    if (!current?.initialized) return;
    const tags = current.publishTags.length
      ? ` and tags ${current.publishTags.join(", ")}`
      : "";
    if (
      !(await confirmAction(
        `Push ${current.mainBranch} and ${current.developBranch}${tags} to origin as one atomic update? The remote must support atomic pushes.`,
      ))
    )
      return;
    const repository = props.repository;
    const currentRequest = ++requestId;
    setBusy(true);
    setError("");
    try {
      await api.gitFlowPublish(repository);
      if (currentRequest !== requestId || repository !== props.repository)
        return;
      setInfo(await api.gitFlowInfo(repository));
      props.onChanged();
      props.onNotice(
        "Gitflow branches and selected release tags published to origin.",
      );
    } catch (reason) {
      if (currentRequest === requestId && repository === props.repository)
        setError(errorMessage(reason));
    } finally {
      if (currentRequest === requestId) setBusy(false);
    }
  }

  return (
    <section>
      <div class="git-tools-heading">
        <h3>Gitflow</h3>
        <span class="muted">
          {info()?.initialized ? "Configured" : "Not set up"}
        </span>
      </div>
      <p class="muted">
        Guided feature, release, and hotfix branches. Finish merges locally,
        then publish the production and development branches together.
      </p>
      <Show when={error()}>
        <p class="tailnet-error" role="alert">
          {error()}
        </p>
      </Show>
      <form class="git-tools-form" onSubmit={saveSettings}>
        <input
          aria-label="Gitflow production branch"
          value={settings().mainBranch}
          onInput={(event) => change("mainBranch", event.currentTarget.value)}
          placeholder="Production branch"
          required
        />
        <input
          aria-label="Gitflow development branch"
          value={settings().developBranch}
          onInput={(event) =>
            change("developBranch", event.currentTarget.value)
          }
          placeholder="Development branch"
          required
        />
        <input
          aria-label="Gitflow feature prefix"
          value={settings().featurePrefix}
          onInput={(event) =>
            change("featurePrefix", event.currentTarget.value)
          }
          placeholder="feature/"
          required
        />
        <input
          aria-label="Gitflow release prefix"
          value={settings().releasePrefix}
          onInput={(event) =>
            change("releasePrefix", event.currentTarget.value)
          }
          placeholder="release/"
          required
        />
        <input
          aria-label="Gitflow hotfix prefix"
          value={settings().hotfixPrefix}
          onInput={(event) => change("hotfixPrefix", event.currentTarget.value)}
          placeholder="hotfix/"
          required
        />
        <input
          aria-label="Gitflow tag prefix"
          value={settings().tagPrefix}
          onInput={(event) => change("tagPrefix", event.currentTarget.value)}
          placeholder="Optional tag prefix, such as v"
        />
        <button class="mini-button" type="submit" disabled={busy()}>
          {info()?.initialized ? "Save settings" : "Set up Gitflow"}
        </button>
      </form>
      <Show when={info()?.initialized}>
        <form class="git-tools-form" onSubmit={start}>
          <select
            aria-label="Gitflow branch type"
            value={kind()}
            onChange={(event) => setKind(event.currentTarget.value)}
          >
            <option value="feature">Feature</option>
            <option value="release">Release</option>
            <option value="hotfix">Hotfix</option>
          </select>
          <input
            aria-label="New Gitflow branch name"
            value={name()}
            onInput={(event) => setName(event.currentTarget.value)}
            placeholder="Branch name"
            required
          />
          <button class="mini-button" type="submit" disabled={busy()}>
            Start branch
          </button>
        </form>
        <Show when={activeKind()}>
          <div class="git-tools-buttons">
            <span class="muted">Current: {info()?.currentBranch}</span>
            <button class="mini-button" disabled={busy()} onClick={finish}>
              Finish {activeKind()}
            </button>
          </div>
        </Show>
        <div class="git-tools-buttons">
          <button
            class="mini-button"
            type="button"
            disabled={busy()}
            onClick={() => void publish()}
          >
            Publish Gitflow
          </button>
          <span class="muted">
            {info()?.publishTags.length
              ? `Includes tags: ${info()?.publishTags.join(", ")}`
              : "No release tags reachable from production"}
          </span>
        </div>
      </Show>
    </section>
  );
}
