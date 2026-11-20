// Backend contract: generated from Rust by ts-rs into ./bindings (do not edit those by hand).
// Sizes are bytes, times are unix seconds.

export type { AppGroup } from "./bindings/AppGroup";
export type { Dashboard } from "./bindings/Dashboard";
export type { HistoryPoint } from "./bindings/HistoryPoint";
export type { JournalEntry } from "./bindings/JournalEntry";
export type { JournalSource } from "./bindings/JournalSource";
export type { KillOutcome } from "./bindings/KillOutcome";
export type { KillPolicy } from "./bindings/KillPolicy";
export type { Pressure } from "./bindings/Pressure";
export type { ProcBrief } from "./bindings/ProcBrief";
export type { ProcessDetails } from "./bindings/ProcessDetails";
export type { ProcessRow } from "./bindings/ProcessRow";
export type { ProcState } from "./bindings/ProcState";
export type { Signal } from "./bindings/Signal";
export type { Suspect } from "./bindings/Suspect";
export type { SuspectKind } from "./bindings/SuspectKind";
export type { SystemMemory } from "./bindings/SystemMemory";
export type { VmCounters } from "./bindings/VmCounters";
export type { Author } from "./bindings/Author";
export type { AuthorLink } from "./bindings/AuthorLink";
export type { Origin } from "./bindings/Origin";
export type { GuardMode } from "./bindings/GuardMode";
export type { GuardZone } from "./bindings/GuardZone";
export type { GuardPressure } from "./bindings/GuardPressure";
export type { GuardRejected } from "./bindings/GuardRejected";
export type { GuardCandidate } from "./bindings/GuardCandidate";
export type { LegacyGuard } from "./bindings/LegacyGuard";
export type { AllowRule } from "./bindings/AllowRule";
export type { RuleSpec } from "./bindings/RuleSpec";
export type { Notice } from "./bindings/Notice";
export type { AppConfig } from "./bindings/AppConfig";
export type { ThresholdSettings } from "./bindings/ThresholdSettings";

import type { KillPolicy } from "./bindings/KillPolicy";
export type { GuardConfig } from "./bindings/GuardConfig";
export type { GuardEvent } from "./bindings/GuardEvent";
export type { GuardLimit } from "./bindings/GuardLimit";
export type { GuardOrphans } from "./bindings/GuardOrphans";
export type { GuardOrphanWatch } from "./bindings/GuardOrphanWatch";
export type { GuardRed } from "./bindings/GuardRed";
export type { GuardSettings } from "./bindings/GuardSettings";
export type { GuardStatus } from "./bindings/GuardStatus";
export type { Settings } from "./bindings/Settings";

// ---- UI-only types ----

/** Arguments of the `kill_process` command (Tauri maps camelCase to snake_case). */
export interface KillArgs {
  pid: number;
  startedAt: number;
  confirm: boolean;
  reason: string;
}

/** What the kill dialog needs to know about the target (built from a Suspect or a ProcessRow). */
export interface KillTarget {
  pid: number;
  name: string;
  app_group: string;
  started_at: number;
  policy: KillPolicy;
  reclaim: number;
  /** Text recorded in the action log as the reason. */
  reason: string;
}

/** Identity of a process: pid alone is reused, (pid, started_at) is not. */
export interface ProcKey {
  pid: number;
  started_at: number;
}
