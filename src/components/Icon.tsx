import type { JSX } from "solid-js";

// Exact Figma exports. Rendered as a CSS mask so one glyph can take any token colour.
const urls = import.meta.glob<string>("../assets/icons/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
});

const byName: Record<string, string> = {};
for (const [path, url] of Object.entries(urls)) {
  byName[path.slice(path.lastIndexOf("/") + 1, -4)] = url;
}

export type IconName =
  | "alert" | "arrow-down" | "arrow-up" | "branch" | "check" | "chevron-down"
  | "chevron-right" | "clock" | "copy" | "database" | "file" | "folder"
  | "folder-plus" | "graph" | "info" | "layers" | "list" | "logo" | "play"
  | "plus" | "power" | "restart" | "search" | "settings" | "split" | "stop"
  | "terminal" | "trash" | "x";

export function Icon(props: {
  name: IconName;
  size: number;
  /** CSS colour; defaults to currentColor */
  color?: string;
  class?: string;
  style?: JSX.CSSProperties;
}) {
  return (
    <span
      class={`icon ${props.class ?? ""}`}
      aria-hidden="true"
      style={{
        width: `${props.size}px`,
        height: `${props.size}px`,
        "background-color": props.color ?? "currentColor",
        "-webkit-mask-image": `url("${byName[props.name]}")`,
        "mask-image": `url("${byName[props.name]}")`,
        ...props.style,
      }}
    />
  );
}
