import type { MouseEvent } from "react";
import type { TargetSlot } from "../types";
import { slotGlyph, targetLabel } from "../services/format";

interface Props {
  target: TargetSlot;
  selected: boolean;
  /** This target receives the utterance currently in progress. */
  receiving?: boolean;
  hasSuggestion?: boolean;
  onSelect?: (target: TargetSlot) => void;
  onActivate?: (target: TargetSlot) => void;
  onContextMenu?: (target: TargetSlot) => void;
  /** Shown as a × on a target whose window is closed. */
  onRemove?: (target: TargetSlot) => void;
}

export function TargetChip({ target, selected, receiving, hasSuggestion, onSelect, onActivate, onContextMenu, onRemove }: Props) {
  const offline = target.status === "offline";
  const classes = ["chip", selected ? "selected" : "", offline ? "offline" : "", receiving ? "receiving" : ""]
    .filter(Boolean)
    .join(" ");
  const title = offline
    ? `${targetLabel(target)} — window closed${hasSuggestion ? " (right-click to rebind)" : ""}`
    : `${targetLabel(target)} — ${target.title_hint || "window"}\nClick: select · Double-click: bring to front`;
  const handleContext = (e: MouseEvent) => {
    e.preventDefault();
    onContextMenu?.(target);
  };
  const chip = (
    <button
      type="button"
      className={classes}
      title={title}
      // Keep keyboard focus where it is: the overlay never takes it.
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => onSelect?.(target)}
      onDoubleClick={() => onActivate?.(target)}
      onContextMenu={handleContext}
    >
      {receiving ? <span className="chip-arrow">→</span> : null}
      <span className="chip-slot">{receiving ? slotGlyph(target.slot) : target.slot}</span>
      <span className="chip-label">{targetLabel(target)}</span>
      {offline ? <span className="chip-flag">{hasSuggestion ? "↻" : "!"}</span> : null}
    </button>
  );
  if (!offline || !onRemove) return chip;
  return (
    <span className="chip-group">
      {chip}
      <button
        type="button"
        className="chip-remove"
        title={`Remove ${targetLabel(target)} (window closed)`}
        aria-label={`Remove ${targetLabel(target)}`}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => onRemove(target)}
      >
        ×
      </button>
    </span>
  );
}
