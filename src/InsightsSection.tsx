import { useState } from "react";
import type { Flow } from "./api";
import type { Insight } from "./bindings/Insight";
import type { InsightReport } from "./bindings/InsightReport";
import type { StatusClass } from "./classify";
import { Icon } from "./icons";
import { INSIGHT_DOCS } from "./insightDocs";
import { HostsTable, ReportHeader } from "./InsightsReport";
import { partition, SEVERITY_LABEL, severityCounts } from "./insights";
import type { AppGroup, TrafficSummary } from "./report";

/**
 * The Insights page: anti-patterns the engine found, most severe first.
 *
 * Each card says what was seen in numbers and what to do about it, and opens
 * the flows behind it in the table — the finding is the start of an
 * investigation, never a verdict to take on trust.
 */
export function InsightsSection({
  report,
  summary,
  hosts,
  inView,
  filterNote,
  canFollow,
  onFollow,
  recording,
  imported,
  muted,
  onToggleMute,
  onShowFlows,
  onRefresh,
  onExport,
  onSelectFlow,
  onStatus,
  onHost,
}: {
  report: InsightReport | null;
  /** The traffic report over the same flows the findings follow. */
  summary: TrafficSummary;
  hosts: AppGroup[];
  /** Per finding, how many of its flows pass the filter (only while following it). */
  inView: ReadonlyMap<string, number>;
  /** Set while the page follows the flow filter: what it says, and what it hides. */
  filterNote: { label: string; hidden: number } | null;
  /** A filter is set but the page was told to show everything. */
  canFollow: boolean;
  onFollow: (follow: boolean) => void;
  recording: boolean;
  imported: boolean;
  muted: ReadonlySet<string>;
  onToggleMute: (i: Insight) => void;
  onShowFlows: (i: Insight) => void;
  onRefresh: () => void;
  onExport: () => void;
  onSelectFlow: (f: Flow) => void;
  onStatus: (c: StatusClass) => void;
  onHost: (app: string, host: string) => void;
}) {
  const [showMuted, setShowMuted] = useState(false);
  const { shown, muted: hidden } = partition(report, muted);
  const counts = severityCounts(shown);
  const analyzed = report?.analyzed ?? 0;

  // Say where the numbers come from and whether they are still moving: a list
  // that silently stopped updating when capture paused reads as "all fixed".
  const source = imported ? "the imported session" : "the current capture";
  const liveness = imported
    ? "Re-checked when the list changes."
    : recording
      ? "Updates live while recording."
      : "Paused — press Refresh to re-check.";

  return (
    <div className="page">
      <div className="page-inner w720">
        <div className="page-head">
          <h2 className="page-title">Insights</h2>
          <div className="page-head-actions">
            <div className="btn-neutral" onClick={onExport} title="Save this report as Markdown — no query strings, header values or bodies">
              <Icon name="file-down" />Export report
            </div>
            <div className="btn-neutral" onClick={onRefresh}><Icon name="refresh-cw" />Refresh</div>
          </div>
        </div>
        <p className="page-sub">
          Anti-patterns found in {analyzed} {analyzed === 1 ? "flow" : "flows"} of {source}. {liveness}
        </p>

        {filterNote && (
          <div className="ins-filter">
            <Icon name="filter" size={13} />
            <span>
              Following the flow filter <b>{filterNote.label}</b>
              {filterNote.hidden > 0 &&
                ` · ${filterNote.hidden} other ${filterNote.hidden === 1 ? "finding" : "findings"} hidden`}
            </span>
            <span className="ins-filter-act" onClick={() => onFollow(false)}>Show all</span>
          </div>
        )}
        {canFollow && (
          <div className="ins-filter">
            <Icon name="filter" size={13} />
            <span>Showing every finding; the flow filter is not applied.</span>
            <span className="ins-filter-act" onClick={() => onFollow(true)}>Follow filter</span>
          </div>
        )}

        {summary.requests > 0 && <ReportHeader summary={summary} onSelectFlow={onSelectFlow} onStatus={onStatus} />}

        <h3 className="rep-h">Findings</h3>
        {shown.length > 0 && (
          <div className="ins-counts">
            {(["High", "Warning", "Info"] as const).map((s) => (
              <span key={s} className={`ins-count ${s.toLowerCase()}`}>
                <span className="dot" />{counts[s]} {SEVERITY_LABEL[s]}
              </span>
            ))}
          </div>
        )}

        {shown.length === 0 && (
          <div className="ins-empty">
            <Icon name="lightbulb" size={22} />
            <div>
              {analyzed === 0
                ? "Nothing analyzed yet. Record some traffic and findings appear here as patterns show up."
                : filterNote
                  ? "Nothing to flag in the flows this filter shows."
                  : `Nothing to flag in ${analyzed} flows${hidden.length ? ` (${hidden.length} muted)` : ""}.`}
            </div>
          </div>
        )}

        {shown.map((i) => (
          <InsightCard key={i.id} insight={i} inView={inView.get(i.id)} onShowFlows={onShowFlows} onToggleMute={onToggleMute} isMuted={false} />
        ))}

        {hidden.length > 0 && (
          <>
            <div className="ins-muted-toggle" onClick={() => setShowMuted(!showMuted)}>
              <Icon name={showMuted ? "chevron-down" : "chevron-right"} size={13} />
              {hidden.length} muted
            </div>
            {showMuted &&
              hidden.map((i) => (
                <InsightCard key={i.id} insight={i} inView={inView.get(i.id)} onShowFlows={onShowFlows} onToggleMute={onToggleMute} isMuted />
              ))}
          </>
        )}

        <HostsTable groups={hosts} onHost={onHost} />
      </div>
    </div>
  );
}

function InsightCard({
  insight: i,
  inView,
  isMuted,
  onShowFlows,
  onToggleMute,
}: {
  insight: Insight;
  /** Flows of this finding the filter lets through, when following one. */
  inView?: number;
  isMuted: boolean;
  onShowFlows: (i: Insight) => void;
  onToggleMute: (i: Insight) => void;
}) {
  const [about, setAbout] = useState(false);
  const doc = INSIGHT_DOCS[i.rule as keyof typeof INSIGHT_DOCS];
  const where = [i.app, i.host].filter(Boolean).join(" · ");
  return (
    <div className={`ins-card ${i.severity.toLowerCase()} ${isMuted ? "muted" : ""}`}>
      <div className="ins-head">
        <span className={`ins-sev ${i.severity.toLowerCase()}`}>{SEVERITY_LABEL[i.severity]}</span>
        <div className="ins-title">{i.title}</div>
        <div className="ins-actions">
          <div className="btn-neutral sm" onClick={() => onShowFlows(i)}>
            <Icon name="list" />
            {i.count === 1 ? "Show flow" : `Show ${i.count} flows`}
          </div>
          {doc && (
            <div className={`ins-mute ${about ? "on" : ""}`} title="About this check" onClick={() => setAbout(!about)}>
              <Icon name="info" size={15} />
            </div>
          )}
          <div
            className="ins-mute"
            title={isMuted ? "Unmute" : "Mute this finding — it stays muted across launches"}
            onClick={() => onToggleMute(i)}
          >
            <Icon name={isMuted ? "bell" : "bell-off"} size={15} />
          </div>
        </div>
      </div>
      <div className="ins-where">
        {where} <span className="ins-rule">{i.rule}</span>
        {inView != null && inView < i.count && (
          <span className="ins-inview">{inView} of {i.count} flows in view</span>
        )}
      </div>
      <div className="ins-detail">{withCode(i.detail)}</div>
      <div className="ins-advice"><Icon name="lightbulb" size={13} /><span>{withCode(i.advice)}</span></div>
      {about && doc && (
        <dl className="ins-about">
          <dt>What it checks</dt><dd>{doc.what}</dd>
          <dt>Why it matters</dt><dd>{doc.why}</dd>
          <dt>How to fix</dt><dd>{doc.fix}</dd>
          <dt>Impact</dt><dd className="impact">{doc.impact}</dd>
        </dl>
      )}
    </div>
  );
}

/** The engine marks names (`access_token`, `Cache-Control: …`) with backticks. */
function withCode(text: string) {
  return text.split("`").map((part, n) => (n % 2 === 1 ? <code key={n}>{part}</code> : part));
}
