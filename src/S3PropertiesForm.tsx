import { createResource, createSignal, For, Show } from "solid-js";
import { Plus, X } from "lucide-solid";
import { api, errorMessage } from "./api";

type Pair = [string, string];

const storageClasses = [
  "STANDARD",
  "INTELLIGENT_TIERING",
  "STANDARD_IA",
  "ONEZONE_IA",
  "GLACIER_IR",
  "GLACIER",
  "DEEP_ARCHIVE",
  "REDUCED_REDUNDANCY",
];

function PairRows(props: {
  label: string;
  rows: Pair[];
  onChange: (rows: Pair[]) => void;
  max?: number;
}) {
  const update = (index: number, column: 0 | 1, value: string) =>
    props.onChange(
      props.rows.map((row, at) =>
        at === index
          ? ((column === 0 ? [value, row[1]] : [row[0], value]) as Pair)
          : row,
      ),
    );
  return (
    <fieldset class="pair-rows wide">
      <legend>{props.label}</legend>
      <For each={props.rows}>
        {(row, index) => (
          <div class="pair-row">
            <input
              aria-label={`${props.label} name`}
              placeholder="Name"
              spellcheck={false}
              value={row[0]}
              onInput={(event) => update(index(), 0, event.currentTarget.value)}
            />
            <input
              aria-label={`${props.label} value`}
              placeholder="Value"
              spellcheck={false}
              value={row[1]}
              onInput={(event) => update(index(), 1, event.currentTarget.value)}
            />
            <button
              type="button"
              class="icon-button"
              aria-label={`Remove ${row[0] || "row"}`}
              onClick={() =>
                props.onChange(props.rows.filter((_, at) => at !== index()))
              }
            >
              <X size={14} />
            </button>
          </div>
        )}
      </For>
      <button
        type="button"
        class="mini-button"
        disabled={props.max !== undefined && props.rows.length >= props.max}
        onClick={() => props.onChange([...props.rows, ["", ""]])}
      >
        <Plus size={13} /> Add
      </button>
    </fieldset>
  );
}

/** Content type, caching, storage class, metadata and tags of an S3 object. */
export default function S3PropertiesForm(props: {
  profileId: string;
  path: string;
  onDone: (message: string) => void;
  onClose: () => void;
}) {
  const [contentType, setContentType] = createSignal("");
  const [cacheControl, setCacheControl] = createSignal("");
  const [disposition, setDisposition] = createSignal("");
  const [encoding, setEncoding] = createSignal("");
  const [storageClass, setStorageClass] = createSignal("STANDARD");
  const [metadata, setMetadata] = createSignal<Pair[]>([]);
  const [tags, setTags] = createSignal<Pair[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [loaded] = createResource(
    () => [props.profileId, props.path] as const,
    async ([profileId, path]) => {
      const properties = await api.s3ObjectProperties(profileId, path);
      setContentType(properties.contentType);
      setCacheControl(properties.cacheControl);
      setDisposition(properties.contentDisposition);
      setEncoding(properties.contentEncoding);
      setStorageClass(properties.storageClass);
      setMetadata(properties.metadata);
      setTags(properties.tags ?? []);
      return properties;
    },
  );

  async function save(event: Event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const clean = (rows: Pair[]) =>
        rows.filter(([name, value]) => name.trim() || value.trim());
      await api.s3ObjectPropertiesSet(
        props.profileId,
        props.path,
        {
          contentType: contentType(),
          cacheControl: cacheControl(),
          contentDisposition: disposition(),
          contentEncoding: encoding(),
          storageClass: storageClass(),
          metadata: clean(metadata()),
        },
        loaded()?.tags === null ? null : clean(tags()),
      );
      props.onDone("S3 properties saved.");
      props.onClose();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Show
      when={!loaded.error}
      fallback={<p class="form-error">{errorMessage(loaded.error)}</p>}
    >
      <Show when={loaded()} fallback={<p class="muted">Loading properties…</p>}>
        {(properties) => (
          <form class="form-grid" onSubmit={save}>
            <p class="muted wide">
              {properties().size.toLocaleString()} bytes
              {properties().lastModified
                ? ` · changed ${properties().lastModified}`
                : ""}
              {properties().encryption
                ? ` · encrypted (${properties().encryption})`
                : ""}
            </p>
            <label>
              Content type
              <input
                spellcheck={false}
                placeholder="application/octet-stream"
                value={contentType()}
                onInput={(event) => setContentType(event.currentTarget.value)}
              />
            </label>
            <label>
              Storage class
              <select
                value={storageClass()}
                onChange={(event) => setStorageClass(event.currentTarget.value)}
              >
                <For
                  each={
                    storageClasses.includes(storageClass())
                      ? storageClasses
                      : [storageClass(), ...storageClasses]
                  }
                >
                  {(name) => <option value={name}>{name}</option>}
                </For>
              </select>
            </label>
            <label>
              Cache control
              <input
                spellcheck={false}
                placeholder="max-age=3600"
                value={cacheControl()}
                onInput={(event) => setCacheControl(event.currentTarget.value)}
              />
            </label>
            <label>
              Content encoding
              <input
                spellcheck={false}
                placeholder="gzip"
                value={encoding()}
                onInput={(event) => setEncoding(event.currentTarget.value)}
              />
            </label>
            <label class="wide">
              Content disposition
              <input
                spellcheck={false}
                placeholder='attachment; filename="report.csv"'
                value={disposition()}
                onInput={(event) => setDisposition(event.currentTarget.value)}
              />
            </label>
            <PairRows
              label="Metadata"
              rows={metadata()}
              onChange={setMetadata}
            />
            <Show
              when={properties().tags !== null}
              fallback={
                <p class="muted wide">This service does not support tags.</p>
              }
            >
              <PairRows
                label="Tags"
                rows={tags()}
                onChange={setTags}
                max={10}
              />
            </Show>
            <Show when={error()}>
              <p class="form-error wide" role="alert">
                {error()}
              </p>
            </Show>
            <div class="wide">
              <button class="primary-button" type="submit" disabled={busy()}>
                {busy() ? "Saving…" : "Save"}
              </button>
            </div>
          </form>
        )}
      </Show>
    </Show>
  );
}
