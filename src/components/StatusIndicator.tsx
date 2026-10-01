import type { Phase } from "../types";
import { phaseGlyph, phaseLabel, phaseTone } from "../services/format";

interface Props {
  phase: Phase;
  /** Extra text after the label, e.g. the elapsed recording time. */
  detail?: string;
}

export function StatusIndicator({ phase, detail }: Props) {
  const tone = phaseTone(phase);
  return (
    <span className={`status tone-${tone}`} data-tauri-drag-region>
      <span className={`status-glyph${phase === "LISTENING" ? " pulse" : ""}`} data-tauri-drag-region>
        {phaseGlyph(phase)}
      </span>
      <span className="status-label" data-tauri-drag-region>
        {phaseLabel(phase)}
      </span>
      {detail ? (
        <span className="status-detail" data-tauri-drag-region>
          {detail}
        </span>
      ) : null}
    </span>
  );
}
