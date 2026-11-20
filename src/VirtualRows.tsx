import {
  createMemo,
  createSignal,
  For,
  onCleanup,
  onMount,
  type JSX,
} from "solid-js";

type Props<T> = {
  items: T[];
  rowHeight: number;
  /** The element that scrolls; by default the nearest scrolling ancestor. */
  scroller?: () => HTMLElement | undefined;
  /** Called when the last rows come into view, for loading more. */
  onNearEnd?: () => void;
  children: (item: T) => JSX.Element;
};

/** Renders only the rows in view, so long lists stay fast. */
function scrollParent(element: HTMLElement): HTMLElement | undefined {
  for (let node = element.parentElement; node; node = node.parentElement)
    if (/(auto|scroll)/.test(getComputedStyle(node).overflowY)) return node;
  return undefined;
}

export default function VirtualRows<T>(props: Props<T>) {
  let list!: HTMLDivElement;
  let scroller: HTMLElement | undefined;
  const [range, setRange] = createSignal({ start: 0, end: 60 });
  const overscan = 10;

  function measure() {
    if (!scroller || !list) return;
    const top =
      list.getBoundingClientRect().top -
      scroller.getBoundingClientRect().top +
      scroller.scrollTop;
    const first = Math.floor((scroller.scrollTop - top) / props.rowHeight);
    const visible = Math.ceil(scroller.clientHeight / props.rowHeight);
    const start = Math.max(0, first - overscan);
    const end = Math.min(props.items.length, first + visible + overscan);
    setRange((current) =>
      current.start === start && current.end === end ? current : { start, end },
    );
    if (props.onNearEnd && end >= props.items.length - overscan)
      props.onNearEnd();
  }

  onMount(() => {
    const element = props.scroller?.() ?? scrollParent(list);
    if (!element) return;
    scroller = element;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    element.addEventListener("scroll", measure, { passive: true });
    onCleanup(() => {
      observer.disconnect();
      element.removeEventListener("scroll", measure);
    });
    measure();
  });

  const visible = createMemo(() => {
    const rows = props.items.slice(range().start, range().end);
    // The list may have grown or shrunk; check the window again after render.
    queueMicrotask(measure);
    return rows;
  });

  return (
    <div
      ref={list}
      class="virtual-rows"
      style={{ height: `${props.items.length * props.rowHeight}px` }}
    >
      <div
        style={{
          transform: `translateY(${range().start * props.rowHeight}px)`,
        }}
      >
        <For each={visible()}>{(item) => props.children(item)}</For>
      </div>
    </div>
  );
}
