import { POLICY_LABEL } from "../format";
import type { KillPolicy, KillTarget } from "../types";
import type { KillRequest } from "./KillDialog";

export function LockIcon() {
  return (
    <svg className="lock" viewBox="0 0 12 14" width="10" height="12" aria-hidden="true">
      <rect x="1" y="6" width="10" height="7.5" rx="1.5" fill="currentColor" />
      <path d="M3.3 6V4.2a2.7 2.7 0 0 1 5.4 0V6" fill="none" stroke="currentColor" strokeWidth="1.6" />
    </svg>
  );
}

/** Compact policy label for tables; full reason on hover. */
export function PolicyTag({ policy }: { policy: KillPolicy }) {
  return (
    <span className={`policy pol-${policy.kind}`} title={policy.kind === "free" ? undefined : policy.reason}>
      {policy.kind === "protected" && <LockIcon />}
      {POLICY_LABEL[policy.kind]}
    </span>
  );
}

/** «Снять» button; for protected processes renders a lock instead (no button at all). */
export function KillButton({ target, onKill }: { target: KillTarget; onKill: (r: KillRequest) => void }) {
  if (target.policy.kind === "protected") {
    return (
      <span className="locked" title={target.policy.reason}>
        <LockIcon />
      </span>
    );
  }
  return (
    <button
      className={target.policy.kind === "confirm" ? "btn btn-kill btn-warn" : "btn btn-kill"}
      onClick={(e) => {
        e.stopPropagation();
        onKill({ targets: [target] });
      }}
    >
      Снять
    </button>
  );
}
