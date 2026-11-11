// The daemon's data as it reaches the window: serde JSON of the engine/ crates' types (wasl-ipc proto.rs,
// wasl-store pairs/history/bin.rs, wasl-engine engine/actions/support.rs, wasl-core model.rs, run.rs).

export type Overall = 'up_to_date' | 'syncing' | 'paused' | 'waiting' | 'needs_you' | 'idle';
export type RunStatus = 'RUNNING' | 'SUCCESS' | 'PARTIAL' | 'FAILED' | 'BLOCKED' | 'SKIPPED' | 'CANCELLED';
export type SyncMode =
  | 'TWO_WAY'
  | 'UPLOAD_ONLY'
  | 'UPLOAD_THEN_DELETE'
  | 'UPLOAD_MIRROR'
  | 'DOWNLOAD_ONLY'
  | 'DOWNLOAD_THEN_DELETE'
  | 'DOWNLOAD_MIRROR';
export type Phase =
  | 'starting'
  | 'listing_remote'
  | 'scanning_local'
  | 'planning'
  | 'transferring'
  | 'deleting'
  | 'finishing';
export type Trigger = 'manual' | 'preview' | 'schedule' | 'instant' | 'automation';
export type PairRoute = 'drive' | 'drive_and_devices' | 'devices';
export type ConflictChoice = 'KeepBoth' | 'KeepDevice' | 'KeepDrive';

export interface Progress {
  pair_id: string;
  run_id: string;
  phase: Phase;
  total_actions: number;
  done_actions: number;
  bytes_total: number;
  bytes_done: number;
  checked: number;
  errors: number;
  current_file: string | null;
}

export interface PairStatus {
  last_run_at: number | null;
  last_success_at: number | null;
  last_status: RunStatus | null;
  last_error: string | null;
  last_error_kind: string | null;
  consecutive_failures: number;
  pending_approval: boolean;
  approve_once: boolean;
  hold_for_preview: boolean;
}

export interface Pair {
  id: string;
  name: string;
  account_id: string | null;
  local_root: string;
  drive_folder_id: string | null;
  remote_path: string;
  method: SyncMode;
  /** Where the pair syncs: Drive only, or also (or only) with this person's other devices. */
  route?: PairRoute;
  enabled: boolean;
  paused: boolean;
  pause_reason: string | null;
  /** The vault the pair syncs through, when its Drive folder is an encrypted vault. */
  vault_id?: string | null;
  status: PairStatus;
}

/** What Edit pair changes (#65): the name and the method, and when given, the folders (#103). A folder change resets
 *  the pair and holds it for a new preview. */
export interface PairEdit {
  name: string;
  method: SyncMode;
  /** Another folder on this computer. */
  localRoot?: string;
  /** Another Google account, Drive folder or vault. */
  drive?: PairDrive;
}

/** A pair's Drive side, as Edit pair sets it. */
export interface PairDrive {
  accountId: string;
  driveFolderId: string;
  remotePath: string;
  driveId: string | null;
  vaultId: string | null;
}

export interface PairState {
  pair: Pair;
  running: boolean;
  progress: Progress | null;
  open_conflicts: number;
  blocked: boolean;
  /** Bytes in the folder at its last sync (from the baseline). Null before the first sync; absent from an older daemon. */
  folder_bytes?: number | null;
  /** Files the last finished run's plan skipped (ignore-rule exclusions not counted). Null before the first finished run. */
  skipped?: number | null;
  /** Other devices the pair syncs with; empty for a Drive-only pair. */
  peers?: PeerState[];
  /** Free bytes on the drive holding the folder. Null when it can't be told. */
  free_bytes?: number | null;
}

/** Another device a pair syncs with ("Waiting for <name>"). */
export interface PeerState {
  id: string;
  name: string;
  online: boolean;
}

export interface Account {
  id: string;
  email: string;
  auth_error: string | null;
}

export interface DaemonStatus {
  /** Agent changesets waiting for a decision. */
  agent_changes?: number;
  version: string;
  protocol: number;
  accounts: Account[];
  pairs: PairState[];
  overall: Overall;
}

export interface PlanSummary {
  uploads: number;
  uploadBytes: number;
  downloads: number;
  downloadBytes: number;
  exports: number;
  deleteLocal: number;
  deleteRemote: number;
  conflicts: number;
  mkdirs: number;
  skips: number;
  localFiles: number;
  remoteFiles: number;
  warnings: string[];
  blockedReason: string | null;
  sampleDeletes: string[];
  unchanged: number;
}

export interface Preview {
  plan: { summary: PlanSummary; blocked: boolean };
  /** What needs the user's attention; the plan's warnings are among them. */
  notes: string[];
  /** Not problems (where the comparison's time went, renames followed, shortcuts left out). */
  infos?: string[];
}

export interface RunOutcome {
  run_id: string;
  status: RunStatus;
  message: string | null;
  summary: PlanSummary | null;
}

export interface RunStats {
  uploaded: number;
  downloaded: number;
  exported: number;
  deleted_local: number;
  deleted_remote: number;
  conflicts: number;
  skipped: number;
  errors: number;
  bytes_up: number;
  bytes_down: number;
}

export interface RunRecord {
  id: string;
  pair_id: string;
  trigger: string;
  status: RunStatus;
  stats: RunStats;
  message: string | null;
  started_at: number;
  ended_at: number | null;
}

export interface RunEvent {
  at: number;
  level: 'Info' | 'Warn' | 'Error';
  action: string;
  key: string | null;
  message: string;
  detail: string | null;
  bytes: number;
}

export interface ConflictRecord {
  id: string;
  pair_id: string;
  key: string;
  detected_at: number;
  message: string;
}

export interface ConflictSide {
  mtime_ms: number;
  size: number;
}

export interface ConflictDetails {
  conflict: ConflictRecord;
  name: string;
  folder: string;
  device: ConflictSide | null;
  drive: ConflictSide | null;
}

/** One file or folder a stopped plan would delete (wasl-engine actions.rs). */
export interface PendingDeletion {
  key: string;
  name: string;
  /** Parent key, "" at the root. */
  folder: string;
  is_dir: boolean;
  /** Files inside a folder; 1 for a file. */
  files: number;
  bytes: number;
  /** The deletion would happen on Drive: the file is gone from this device. */
  on_drive: boolean;
}

/** Everything a stopped plan would delete ("Review deletions"). */
export interface DeletionReview {
  summary: PlanSummary;
  /** Folders first, then files, by path. */
  items: PendingDeletion[];
  files_on_drive: number;
  files_on_device: number;
  bytes: number;
}

/** Opaque: handed back to undo_conflict as it came. */
export type ConflictUndo = Record<string, unknown>;

export interface BinEntry {
  entry: {
    id: string;
    pair_id: string;
    side: 'Local' | 'Drive';
    key: string;
    reason: string;
    location: string;
    deleted_at: number;
    expires_at: number;
    size: number;
    display_name: string | null;
    pair_name: string | null;
  };
  version_no: number;
}

/** An agent's session and what it changed (docs/AGENT_CHANGES.md). */
export type ChangesetStatus = 'open' | 'held' | 'accepted' | 'reverted' | 'partly_reverted';

export interface Changeset {
  id: string;
  pair_id: string;
  actor: string;
  label: string | null;
  scope: string;
  session: string | null;
  status: ChangesetStatus;
  started_at: number;
  ended_at: number | null;
  decided_at: number | null;
}

export interface AgentEntry {
  rel: string;
  key: string;
  kind: 'added' | 'changed' | 'deleted';
  is_dir: boolean;
  before_size: number | null;
  before_mtime: number | null;
  after_size: number | null;
  after_mtime: number | null;
  source: 'saved' | 'synced' | 'none';
  location: string | null;
  outcome: string | null;
}

export interface ChangesetDetails {
  changeset: Changeset;
  pair_name: string;
  entries: AgentEntry[];
}

export interface AgentSetup {
  /** The wasl command line installed with the app. */
  cli: string;
  /** Claude Code's hooks block, ready for settings.json. */
  claude_code_hooks: string;
}

export interface RevertReport {
  reverted: string[];
  skipped: string[];
  failed: Array<[string, string]>;
}

export interface RestoreResult {
  restored: BinEntry[];
  failed: Array<{ entry_id: string; message: string }>;
}

export interface DriveFolder {
  id: string;
  name: string;
  drive_id: string | null;
}

export type DaemonEvent =
  | { event: 'progress'; progress: Progress }
  | { event: 'run_finished'; pair_id: string; outcome: RunOutcome }
  | { event: 'changed' }
  | { event: 'account_added'; account: Account }
  | { event: 'account_failed'; message: string }
  | { event: 'shutting_down' };

export type Conn =
  | { state: 'connecting' }
  | { state: 'starting' }
  | { state: 'connected'; version: string }
  | { state: 'not_running'; message: string | null };

export interface UiError {
  kind: string;
  message: string;
}

export interface AppInfo {
  version: string;
  cli: boolean;
}

// ---------------------------------------------------------------- devices (wasl-ipc proto.rs)

export interface DeviceInfo {
  /** `WASL-XXXX-…`. */
  id: string;
  name: string;
  this_device: boolean;
  connected: boolean;
  /** How the connection reached it, while connected. */
  path: 'lan' | 'direct' | 'punched' | null;
  /** Unix seconds. */
  added_at: number;
  /** The device that added it (`WASL-…`), null for the first device or from a service before 0.4. */
  added_by?: string | null;
  /** False while a device another one added waits for the person here to keep it: until then it gets
   * nothing shared with "all my devices" (docs/INTRODUCTIONS.md §3.1). True from a service before 0.4. */
  known?: boolean;
  /** What the device is, for its icon. Not sent by the engine yet (wasl-devices `DeviceInfo` has no kind), so a
   *  device without one is drawn as a computer. */
  kind?: 'desktop' | 'laptop' | 'phone' | null;
}

/** A folder another device offers this one (docs/INTRODUCTIONS.md §3.3). */
export interface OfferInfo {
  share: string;
  from_device: string;
  from_name: string;
  name: string;
  files: number;
  bytes: number;
  /** Unix ms. */
  first_seen: number;
  status: 'new' | 'declined' | 'accepted';
}

/** Who a pair is shared with (docs/INTRODUCTIONS.md §3.2). */
export interface PairSharing {
  share: string;
  /** Every device known on this computer. */
  all_devices: boolean;
  /** Devices it is shared with by name (`WASL-…`). */
  devices: string[];
}

export interface DevicesInfo {
  /** Device sync runs in this daemon. */
  enabled: boolean;
  /** Where other devices reach this one. */
  listen: string | null;
  devices: DeviceInfo[];
}

export type PairingStage = 'idle' | 'waiting' | 'confirm' | 'paired' | 'failed';

export interface PairingStatus {
  stage: PairingStage;
  /** This computer made the code. */
  inviting: boolean;
  /** The code to show (inviter, while waiting). */
  code: string | null;
  /** Unix seconds. */
  expires_at: number | null;
  /** The four words both screens must show. */
  words: string[];
  /** The other device (`WASL-…`), once known. */
  peer_id?: string | null;
  peer_name: string | null;
  /** The person here already confirmed; waiting for the other device. */
  confirmed_here: boolean;
  message: string | null;
}

// ---------------------------------------------------------------- read-only views (wasl-ipc proto.rs, wasl-store views.rs)

export interface View {
  id: string;
  name: string;
  account_id: string;
  drive_folder_id: string;
  drive_id: string | null;
  /** Display path of the Drive folder, e.g. `My Drive/Photos`. */
  remote_path: string;
  /** Where the view appears on this computer. */
  local_path: string;
  vault_id: string | null;
  /** Bytes of unpinned content the view may keep. */
  cache_limit: number;
  enabled: boolean;
  created_at: number;
  last_refresh_at: number | null;
  last_error: string | null;
}

export interface LocalChange {
  /** Relative to the view's folder, `/`-separated. */
  path: string;
  /** Why, in English (with any OS error). */
  reason: string;
  /** Why, as a kind the app words itself (wasl-ipc `LocalChangeKind`). */
  kind?: 'only_here' | 'changed_here' | 'differs' | 'unsupported' | 'failed' | 'restored' | null;
}

/**
 * An accept waits for the person (wasl-ipc `AcceptConfirmation`): the service opened a Wasl
 * dialog showing `code`, and accepts only if they confirm there within `expires_in_secs`. When
 * `retry_in_secs` is set no dialog opened: this changeset's last confirmation was declined or
 * timed out, and it may be asked for again after that long.
 */
export interface AcceptConfirmation {
  code: string;
  expires_in_secs: number;
  retry_in_secs?: number | null;
}

export interface ViewState {
  view: View;
  /** Its folder is set up and serving files. */
  mounted: boolean;
  files: number;
  bytes: number;
  /** Content on this computer: all of it, and the pinned part. */
  cache_used: number;
  pinned_bytes: number;
  /** Things in the folder Wasl left alone: never uploaded, never deleted. */
  local_changes: LocalChange[];
  /** The kind of `view.last_error` (wasl-ipc `ErrorKind`, snake_case), when the service knows it. */
  last_error_kind?: string | null;
}

export interface ViewsInfo {
  /** False where files on demand aren't available (macOS without the File Provider extension). */
  supported: boolean;
  views: ViewState[];
}

// ---------------------------------------------------------------- encrypted vaults (wasl-ipc proto.rs)

export interface VaultInfo {
  id: string;
  name: string;
  /** 1 or 2 (docs/ENCRYPTION.md). */
  version: number;
  /** Its keys are on this computer: its pairs sync and its views show files. */
  unlocked: boolean;
  account_id: string | null;
  drive_folder_id: string | null;
  drive_id: string | null;
  remote_path: string;
  created_at: number;
}
