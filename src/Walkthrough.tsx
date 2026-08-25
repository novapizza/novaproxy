/**
 * The first-run walkthrough: a four-step wizard, then two coachmarks.
 *
 * The wizard *offers* each step and reports whether it is already satisfied; it
 * never performs one on its own. Two of the four raise an OS trust or password
 * prompt, and the app's standing rule is that those only ever follow a click —
 * the same reason the crash-recovery banner is a button and not an automatic
 * elevation.
 *
 * Which steps exist and when each counts as done lives in `onboarding.ts`, so
 * the decisions are testable without a window. This file is the view.
 */

import { useEffect, useLayoutEffect, useState } from "react";
import { api, type CaStatus, type HelperStatus, type ProxyStatus } from "./api";
import { Icon } from "./icons";
import { useStore } from "./store";
import {
  allComplete,
  firstIncompleteStep,
  stepState,
  visibleSteps,
  type Statuses,
  type StepId,
} from "./onboarding";
import { trustHint } from "./trust";

/* -------------------------------- wizard -------------------------------- */

export function OnboardingWizard({
  helper,
  setHelper,
  ca,
  proxy,
  recording,
  flowCount,
  setRecording,
  showToast,
  onDismiss,
}: {
  helper: HelperStatus | null;
  setHelper: (h: HelperStatus) => void;
  ca: CaStatus | null;
  proxy: ProxyStatus;
  recording: boolean;
  flowCount: number;
  setRecording: (v: boolean) => void;
  /** Runs the step, and is what makes the wizard show live status afterwards. */
  showToast: (t: string) => void;
  /** Called with the step to coach next, or null when there is nothing to point at. */
  onDismiss: (coach: boolean) => void;
}) {
  const statuses: Statuses = { helper, ca, proxy, recording, flowCount };
  const steps = visibleSteps(helper);

  // Where to open: the first thing left to do. Computed once — re-deriving it
  // as steps complete would yank the wizard forward under the user's cursor
  // while an install is still settling.
  const [at, setAt] = useState(() => firstIncompleteStep(steps, statuses));
  const [busy, setBusy] = useState(false);

  const index = Math.min(at, steps.length - 1);
  const step = steps[index];
  const state = stepState(step.id, statuses);
  const last = index === steps.length - 1;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onDismiss(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onDismiss]);

  /** Perform the current step. Every failure is the user's to see, not a silent no-op. */
  async function run() {
    setBusy(true);
    try {
      switch (step.id) {
        case "helper": {
          setHelper(await api.installHelper());
          showToast("Helper installed — no more password prompts");
          break;
        }
        case "certificate": {
          useStore.getState().setCa(await api.installCa());
          showToast("Certificate installed & trusted for your user");
          break;
        }
        case "proxy": {
          useStore.getState().setProxy(await api.setSystemProxy(true));
          showToast("System proxy enabled");
          break;
        }
        case "capture": {
          setRecording(true);
          break;
        }
      }
      // Land on the next unfinished step rather than simply index + 1, so a
      // step that was already done does not need a second click to pass.
      if (step.id === "capture") onDismiss(true);
      else setAt(index + 1);
    } catch (e) {
      showToast(String(e));
    } finally {
      setBusy(false);
    }
  }

  const done = allComplete(steps, statuses);

  return (
    <div className="scrim modal-scrim" onClick={() => onDismiss(false)}>
      <div className="modal ob" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Getting started</h2>
          <span className="modal-x" title="Close" onClick={() => onDismiss(false)}>
            <Icon name="x" size={16} />
          </span>
        </div>

        <div className="ob-rail">
          {steps.map((s, i) => {
            const st = stepState(s.id, statuses);
            return (
              <div
                key={s.id}
                className={`ob-pip ${i === index ? "active" : ""} ${st.done ? "done" : ""}`}
                title={s.title}
                onClick={() => setAt(i)}
              >
                <span className="ob-pip-mark">
                  {st.done ? <Icon name="check" size={12} /> : i + 1}
                </span>
                <span className="ob-pip-label">{s.title}</span>
              </div>
            );
          })}
        </div>

        <div className="modal-body ob-body">
          <div className="ob-step">
            <span className={`ob-icon ${state.done ? "done" : ""}`}>
              <Icon name={step.icon} size={22} />
            </span>
            <div className="ob-copy">
              <div className="ob-title">{step.title}</div>
              <div className="ob-state">
                <span className={`dot ${state.done ? "ok" : "warn"}`} />
                {stepStatusLine(step.id, state.done, statuses)}
              </div>
            </div>
          </div>

          <p>{step.blurb}</p>
          {step.id === "certificate" && ca && <p className="ob-fine">{trustHint(ca)}</p>}
          {state.note && <p className="warn-note">{state.note}</p>}
        </div>

        <div className="ob-foot">
          <span className="ob-count">
            Step {index + 1} of {steps.length}
          </span>
          <span className="spacer" />
          {index > 0 && (
            <div className="btn-neutral" onClick={() => !busy && setAt(index - 1)}>
              Back
            </div>
          )}
          {state.done ? (
            <div
              className="btn-primary"
              onClick={() => (last ? onDismiss(!done) : setAt(index + 1))}
            >
              {last ? "Finish" : "Next"}
            </div>
          ) : (
            <>
              <div className="btn-neutral" onClick={() => !busy && (last ? onDismiss(true) : setAt(index + 1))}>
                Skip
              </div>
              <div
                className={`btn-primary ${busy || !canRun(step.id, helper) ? "disabled" : ""}`}
                onClick={() => !busy && canRun(step.id, helper) && void run()}
              >
                {busy ? "Working…" : step.action}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/** One sentence of live status per step — what the user would otherwise go looking for. */
function stepStatusLine(id: StepId, done: boolean, s: Statuses): string {
  switch (id) {
    case "helper":
      return done ? "Installed — proxy changes apply silently" : "Not installed";
    case "certificate":
      return done ? "Trusted" : "Not trusted — HTTPS bodies stay encrypted";
    case "proxy":
      return done ? `Your apps are going through ${s.proxy.host}:${s.proxy.port}` : "Off — the OS is untouched";
    case "capture":
      return s.flowCount > 0
        ? `${s.flowCount} flow${s.flowCount === 1 ? "" : "s"} captured`
        : s.recording
          ? "Recording — nothing has come through yet"
          : "Paused";
  }
}

/** The only step that can be un-runnable: a build tree with no helper binary. */
function canRun(id: StepId, helper: HelperStatus | null): boolean {
  if (id !== "helper") return true;
  return !!helper?.installable;
}

/* ------------------------------- coachmarks ------------------------------- */

export type CoachTarget = "record" | "list" | null;

/** Kept in step with `.coach` in `styles.css`; the clamp needs it before layout. */
const COACH_W = 264;

/**
 * A bubble pinned to an element already on screen.
 *
 * Positioned inside `.nova` (which is `position: relative`) rather than through
 * a portal, and deliberately without a scrim: the whole point is that the user
 * can click the very control it is pointing at.
 */
export function Coachmark({
  anchor,
  text,
  cta,
  onDismiss,
  placement = "below",
}: {
  anchor: React.RefObject<HTMLElement | null>;
  text: string;
  cta: string;
  onDismiss: () => void;
  placement?: "below" | "right";
}) {
  const [box, setBox] = useState<{ top: number; left: number } | null>(null);

  useLayoutEffect(() => {
    const place = () => {
      const el = anchor.current;
      const root = el?.closest(".nova");
      if (!el || !root) return setBox(null);
      const a = el.getBoundingClientRect();
      const r = root.getBoundingClientRect();
      const top = placement === "below" ? a.bottom - r.top + 10 : a.top - r.top + 12;
      const wanted = placement === "below" ? a.left - r.left : a.right - r.left + 12;
      // Keep it inside the window: `.nova` is `overflow: hidden`, so a bubble
      // that runs off the edge is simply not there.
      const left = Math.max(12, Math.min(wanted, r.width - COACH_W - 12));
      setBox({ top, left });
    };
    place();
    window.addEventListener("resize", place);
    // The rail, the splitter and the toolbar all move things around without a
    // resize event; an observer on the root catches those too.
    const root = anchor.current?.closest(".nova");
    const ro = root ? new ResizeObserver(place) : null;
    if (root && ro) ro.observe(root);
    return () => {
      window.removeEventListener("resize", place);
      ro?.disconnect();
    };
  }, [anchor, placement, text]);

  if (!box) return null;

  return (
    <div
      className={`coach ${placement}`}
      style={{ top: box.top, left: box.left }}
      role="status"
    >
      <span className="coach-arrow" />
      <div className="coach-text">{text}</div>
      <div className="coach-foot">
        <span className="coach-btn" onClick={onDismiss}>
          {cta}
        </span>
      </div>
    </div>
  );
}
