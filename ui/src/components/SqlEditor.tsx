import { useMemo } from "react";
import { tokenizeSqlSync } from "../lib/tauriBridge";
import type { LintIssue } from "../lib/types";

interface SqlEditorProps {
  sql: string;
  lintIssues: LintIssue[];
  isRunning: boolean;
  onChange: (next: string) => void;
  onRun: (forceDestructive?: boolean) => void;
  onFormat: () => void;
  onExplain: () => void;
  onOpenExport: () => void;
}

export function SqlEditor({
  sql,
  lintIssues,
  isRunning,
  onChange,
  onRun,
  onFormat,
  onExplain,
  onOpenExport,
}: SqlEditorProps) {
  const tokens = useMemo(() => tokenizeSqlSync(sql), [sql]);
  const lineCount = useMemo(() => Math.max(1, sql.split("\n").length), [sql]);
  const hasBlockingError = lintIssues.some((issue) => issue.severity === "error");

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key === "Enter") {
      e.preventDefault();
      if (!hasBlockingError) {
        onRun(false);
      }
      return;
    }
    if (mod && e.shiftKey && e.key.toLowerCase() === "f") {
      e.preventDefault();
      onFormat();
    }
  };

  return (
    <section className="editor-pane" aria-label="SQL Query Editor">
      <div className="editor-toolbar">
        <div className="toolbar-group">
          <button
            type="button"
            className={hasBlockingError ? "btn btn-danger" : "btn btn-primary"}
            disabled={isRunning || !sql.trim()}
            onClick={() => onRun(hasBlockingError)}
          >
            <span>{isRunning ? "Running..." : hasBlockingError ? "Run Anyway (Destructive)" : "Run Query"}</span>
            <span className="kbd">Ctrl+Enter</span>
          </button>
          <button type="button" className="btn" onClick={onFormat}>
            <span>Format SQL</span>
            <span className="kbd">Ctrl+Shift+F</span>
          </button>
          <button type="button" className="btn" onClick={onExplain} disabled={!sql.trim()}>
            Explain
          </button>
        </div>
        <div className="toolbar-group">
          <button type="button" className="btn" onClick={onOpenExport}>
            Streaming Export
          </button>
        </div>
      </div>

      <div className="editor-body">
        <div className="line-gutter mono" aria-hidden="true">
          {Array.from({ length: lineCount }, (_, idx) => (
            <div key={idx + 1}>{idx + 1}</div>
          ))}
        </div>

        <div className="editor-stage">
          <pre className="sql-highlight mono" aria-hidden="true">
            {tokens.map((tok, idx) => (
              <span key={`${tok.start}-${idx}`} className={`tok-${tok.kind}`}>
                {tok.text}
              </span>
            ))}
            {sql.endsWith("\n") ? " " : ""}
          </pre>
          <textarea
            className="sql-input"
            value={sql}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={handleKeyDown}
            spellCheck={false}
            autoCapitalize="off"
            autoComplete="off"
            autoCorrect="off"
            aria-label="SQL input editor"
            placeholder="Write SQL here..."
          />
        </div>
      </div>

      <div className="lint-banner" role="status" aria-live="polite">
        {lintIssues.length === 0 ? (
          <div className="lint-clean">
            <span>Safety Linter: PASS — 0 destructive query warnings</span>
            <span className="mono" style={{ color: "var(--text-muted)", fontSize: 11 }}>
              {tokens.filter((t) => t.kind !== "whitespace").length} tokens
            </span>
          </div>
        ) : (
          lintIssues.map((issue, idx) => (
            <div key={`${issue.code}-${idx}`} className="lint-item">
              <span className={`lint-badge ${issue.severity}`}>{issue.code}</span>
              <div>
                <span>{issue.message}</span>{" "}
                <span style={{ color: "var(--text-muted)" }}>{issue.suggestion}</span>
              </div>
            </div>
          ))
        )}
      </div>
    </section>
  );
}
