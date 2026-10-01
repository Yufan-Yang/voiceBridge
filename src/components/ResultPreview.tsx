import type { AppError, OutputKind, UtteranceResult } from "../types";
import { OUTPUT_LABELS, charCount, resultText, timingDetail } from "../services/format";

interface Props {
  result: UtteranceResult | null;
  notice: string | null;
  error: AppError | null;
  busy: boolean;
  /** Output injected by default for this utterance's target. */
  defaultKind: OutputKind;
  onCopy: (kind: OutputKind) => void;
  onInject: (kind: OutputKind) => void;
  onRecompile: () => void;
  onDismiss: () => void;
}

const KINDS: OutputKind[] = ["raw", "normalized", "prompt"];

export function ResultPreview({ result, notice, error, busy, defaultKind, onCopy, onInject, onRecompile, onDismiss }: Props) {
  return (
    <div className="preview">
      {error ? (
        <div className="banner banner-error" role="alert">
          <strong>{error.message}</strong>
          <span>{error.recovery}</span>
          <span className="banner-code">{error.code}</span>
        </div>
      ) : null}
      {notice ? <div className="banner banner-warning">{notice}</div> : null}
      {result?.needs_confirmation ? (
        <div className="banner banner-warning">
          The prompt needs your confirmation before it is injected.
          {result.uncertain_identifiers.length > 0
            ? ` Uncertain identifiers: ${result.uncertain_identifiers.join(", ")}`
            : ""}
        </div>
      ) : null}

      {result ? <div className="timing">{timingDetail(result)}</div> : null}

      {result ? (
        KINDS.map((kind) => {
          const text = resultText(result, kind);
          return (
            <section key={kind} className="variant">
              <header>
                <span className="variant-title">
                  {OUTPUT_LABELS[kind]}
                  {kind === defaultKind ? <span className="variant-default">default</span> : null}
                </span>
                <span className="variant-meta">{charCount(text)} chars</span>
                <button type="button" disabled={!text} onClick={() => onCopy(kind)}>
                  Copy
                </button>
                <button type="button" disabled={!text || busy} onClick={() => onInject(kind)}>
                  Inject
                </button>
              </header>
              <pre className={text ? "" : "empty"}>
                {text || (kind === "prompt" ? "Not written (skipped for speed). Press “Recompile prompt” to create it." : "—")}
              </pre>
            </section>
          );
        })
      ) : (
        <p className="preview-empty">No transcription yet. Hold the Push-to-Talk shortcut and speak.</p>
      )}

      <footer>
        <button type="button" disabled={!result || busy} onClick={onRecompile}>
          Recompile prompt
        </button>
        <button type="button" onClick={onDismiss}>
          Close
        </button>
      </footer>
    </div>
  );
}
