import type { Tone } from "../services/format";

interface Props {
  tone: Tone;
  /**
   * 0..1 for a real, measurable value (recording time against the maximum).
   * Omit for work with no measurable completion: the bar is then
   * indeterminate and never shows an invented percentage.
   */
  value?: number;
}

export function ProgressBar({ tone, value }: Props) {
  const determinate = typeof value === "number";
  return (
    <span
      className={`progress tone-${tone}${determinate ? "" : " indeterminate"}`}
      role="progressbar"
      aria-valuemin={determinate ? 0 : undefined}
      aria-valuemax={determinate ? 100 : undefined}
      aria-valuenow={determinate ? Math.round(value * 100) : undefined}
      data-tauri-drag-region
    >
      <span className="progress-fill" style={determinate ? { width: `${Math.round(value * 100)}%` } : undefined} />
    </span>
  );
}
