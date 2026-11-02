// Generated from the console's Rust types by crates/console/src/api.rs. Do not edit.

export type Board = { 
/**
 * The controller revision this board shows.
 */
revision: number, built_at_ms: number, flow: Flow, 
/**
 * Ordered by id.
 */
cards: Array<Card>, unassigned_incidents: Array<CardIncident>, };

/**
 * What changed between two boards: replace or add `upserts` by id, drop
 * `removals`, and take the rest as given.
 */
export type BoardPatch = { revision: number, built_at_ms: number, flow: Flow, upserts: Array<Card>, removals: Array<number>, unassigned_incidents: Array<CardIncident>, };

export type Card = { 
/**
 * The request's node id.
 */
id: number, 
/**
 * `owner/name`.
 */
repo: string, issue: number, title: string, column: Column, phase: string, ending: Ending | null, waiting_on: string, role: string | null, model: string | null, attempt: number, round: number, retries: Retries, 
/**
 * None until ingest sets up the request's credit.
 */
credit: number | null, 
/**
 * When the request entered its current phase, in Unix milliseconds.
 */
phase_since_ms: number | null, last_progress: string | null, incidents: Array<CardIncident>, badges: Array<Badge>, };

export type CardIncident = { id: string, summary: string, };

/**
 * A workflow stage on the board, in board order.
 */
export type Column = "Backlog" | "Intake" | "Planning" | "Making" | "Checking" | "Reviewing" | "Merging" | "NeedsRob" | "Done";

export type ColumnCount = { column: Column, label: string, count: number, };

export type Badge = "Paused" | "Incident" | "MakerBlocked" | "NeedsRob" | "WaitingForCapacity";

/**
 * How a request ended, or is ending while it cleans up.
 */
export type Ending = "Merged" | "Stopped" | "Replaced" | "Duplicate" | "AlreadyDone" | "NotActionable";

export type Flow = { 
/**
 * One entry per column, in board order.
 */
columns: Array<ColumnCount>, 
/**
 * Requests that haven't reached Done.
 */
in_flight: number, 
/**
 * Merged requests, by when they entered cleanup or closed.
 */
merged_last_hour: number, merged_last_day: number, };

export type Retries = { plan: number, audit: number, run_failures: number, effect_failures: number, };

/**
 * The stream's `database` event: whether the console can read the store.
 */
export type DatabaseStatus = { connected: boolean, };

/**
 * The stream's `patch` event: apply `patch` to the board whose event id is
 * `base`. A client holding another board reconnects for a fresh snapshot.
 */
export type PatchEvent = { base: string, patch: BoardPatch, };
