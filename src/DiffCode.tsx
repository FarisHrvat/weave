import { createResource, createSignal, For, Show } from "solid-js";
import { api, errorMessage } from "./api";
import { sideBySideRows, splitDiff, type SideBySideRow } from "./gitDiff";
import {
  highlight,
  isImage,
  languageFor,
  pairedWordDiffs,
  segmentsHtml,
  splitFiles,
  wordDiff,
  type Segment,
} from "./diffRender";

function stored(key: string, fallback: string) {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // The choice still applies until Weave restarts.
  }
}

const [mode, setModeSignal] = createSignal<"inline" | "split">(
  stored("weave-diff-mode", "inline") === "split" ? "split" : "inline",
);
const [wrap, setWrapSignal] = createSignal(
  stored("weave-diff-wrap", "1") === "1",
);

/** How diffs are shown everywhere in Weave: inline or side by side, wrapped or not. */
export const diffPrefs = {
  mode,
  wrap,
  setMode(value: "inline" | "split") {
    setModeSignal(value);
    store("weave-diff-mode", value);
  },
  toggleWrap() {
    setWrapSignal(!wrap());
    store("weave-diff-wrap", wrap() ? "1" : "0");
  },
};

export function DiffPrefsBar() {
  return (
    <div class="diff-mode">
      <button
        type="button"
        class={mode() === "inline" ? "selected" : ""}
        onClick={() => diffPrefs.setMode("inline")}
      >
        Inline
      </button>
      <button
        type="button"
        class={mode() === "split" ? "selected" : ""}
        onClick={() => diffPrefs.setMode("split")}
      >
        Side by side
      </button>
      <button
        type="button"
        class={wrap() ? "selected" : ""}
        aria-pressed={wrap()}
        title="Wrap long lines"
        onClick={() => diffPrefs.toggleWrap()}
      >
        Wrap
      </button>
    </div>
  );
}

/** One line of code, highlighted, with changed words marked when known. */
export function CodeText(props: {
  text: string;
  language?: string;
  segments?: Segment[];
}) {
  return (
    <span
      class="code-text"
      innerHTML={
        props.segments
          ? segmentsHtml(props.segments, props.language)
          : highlight(props.text, props.language)
      }
    />
  );
}

export function hunkLines(hunk: string) {
  return hunk.split("\n").slice(1).filter(Boolean);
}

/** Rows of a side-by-side hunk with the changed words of each pair. */
type SplitRow = SideBySideRow & {
  leftSegments?: Segment[];
  rightSegments?: Segment[];
};

export function splitRows(hunk: string): SplitRow[] {
  return sideBySideRows(hunk).map((row) => {
    if (
      !row.changed ||
      row.leftNumber === undefined ||
      row.rightNumber === undefined
    )
      return { ...row };
    const words = wordDiff(row.left, row.right);
    return { ...row, leftSegments: words.before, rightSegments: words.after };
  });
}

function Hunk(props: { hunk: string; language?: string }) {
  const lines = () => hunkLines(props.hunk);
  const pairs = () => pairedWordDiffs(lines());
  return (
    <div class="diff-hunk">
      <pre class="diff-section-heading">{props.hunk.split("\n", 1)[0]}</pre>
      <Show
        when={mode() === "inline"}
        fallback={
          <div class="side-by-side">
            <For each={splitRows(props.hunk)}>
              {(row) => (
                <div class="side-by-side-row">
                  <div class={row.changed ? "removed" : ""}>
                    <span>{row.leftNumber ?? ""}</span>
                    <pre>
                      <CodeText
                        text={row.left}
                        language={props.language}
                        segments={row.leftSegments}
                      />
                    </pre>
                  </div>
                  <div class={row.changed ? "added" : ""}>
                    <span>{row.rightNumber ?? ""}</span>
                    <pre>
                      <CodeText
                        text={row.right}
                        language={props.language}
                        segments={row.rightSegments}
                      />
                    </pre>
                  </div>
                </div>
              )}
            </For>
          </div>
        }
      >
        <For each={lines()}>
          {(line, index) => (
            <div
              class={`diff-line ${line.startsWith("+") ? "added" : line.startsWith("-") ? "removed" : ""}`}
            >
              <pre>
                {line[0]}
                <CodeText
                  text={line.slice(1)}
                  language={props.language}
                  segments={pairs().get(index())}
                />
              </pre>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}

/** A diff to read, with highlighting; several files are shown one after another. */
export function ReadOnlyDiff(props: { diff: string; empty?: string }) {
  const files = () => splitFiles(props.diff);
  return (
    <div class={`diff-view ${wrap() ? "wrap" : ""}`}>
      <Show
        when={files().length}
        fallback={<pre>{props.diff || props.empty || "No changes."}</pre>}
      >
        <For each={files()}>
          {(file) => {
            const parts = () => splitDiff(file.body);
            return (
              <section class="diff-file">
                <Show when={files().length > 1}>
                  <div class="diff-file-name">{file.path}</div>
                </Show>
                <Show
                  when={parts().hunks.length}
                  fallback={<pre>{file.header}</pre>}
                >
                  <For each={parts().hunks}>
                    {(hunk) => (
                      <Hunk hunk={hunk} language={languageFor(file.path)} />
                    )}
                  </For>
                </Show>
              </section>
            );
          }}
        </For>
      </Show>
    </div>
  );
}

function size(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/**
 * An image before and after a change. A revision is a commit or branch, ":"
 * for the staged copy, "" for the file on disk, or null when there is none.
 */
export function ImageCompare(props: {
  path: string;
  file: string;
  oldFile?: string | null;
  before: string | null;
  after: string | null;
}) {
  const none = { dataUrl: null, size: 0 };
  const [versions] = createResource(
    () =>
      [
        props.path,
        props.file,
        props.oldFile,
        props.before,
        props.after,
      ] as const,
    async ([path, file, oldFile, before, after]) => {
      const [left, right] = await Promise.all([
        before === null
          ? none
          : api.gitImageVersion(path, before, oldFile || file),
        after === null ? none : api.gitImageVersion(path, after, file),
      ]);
      return { left, right };
    },
  );
  return (
    <Show
      when={!versions.error}
      fallback={<p class="form-error">{errorMessage(versions.error)}</p>}
    >
      <Show when={versions()} fallback={<p class="muted">Loading images…</p>}>
        {(loaded) => (
          <div class="image-compare">
            <For
              each={
                [
                  ["Before", loaded().left],
                  ["After", loaded().right],
                ] as const
              }
            >
              {([label, version]) => (
                <figure>
                  <figcaption>
                    {label}
                    <Show when={version.dataUrl}> · {size(version.size)}</Show>
                  </figcaption>
                  <Show
                    when={version.dataUrl}
                    fallback={<div class="image-missing">No image</div>}
                  >
                    <img
                      src={version.dataUrl!}
                      alt={`${label}: ${props.file}`}
                    />
                  </Show>
                </figure>
              )}
            </For>
          </div>
        )}
      </Show>
    </Show>
  );
}

export { isImage };
