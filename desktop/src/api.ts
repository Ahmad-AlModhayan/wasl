// The window's calls into the Rust side (src-tauri/src/commands.rs), which forwards them to wasld.

import { invoke } from '@tauri-apps/api/core';
export type UpdateCheck = { state: 'current' } | { state: 'newer'; version: string; url: string } | { state: 'failed' };

import type { Chrome } from './chrome';
import type { MenuText } from './chrome/menu';
import type {
  PairEdit,
  Account,
  AcceptConfirmation,
  AgentSetup,
  AppInfo,
  BinEntry,
  Changeset,
  ChangesetDetails,
  ConflictChoice,
  ConflictDetails,
  ConflictRecord,
  ConflictUndo,
  Conn,
  DeletionReview,
  DaemonStatus,
  DevicesInfo,
  DriveFolder,
  OfferInfo,
  Pair,
  PairingStatus,
  PairSharing,
  Preview,
  RestoreResult,
  RevertReport,
  RunEvent,
  RunOutcome,
  RunRecord,
  SyncMode,
  Trigger,
  UiError,
  VaultInfo,
  View,
  ViewsInfo,
} from './types';

export function isUiError(e: unknown): e is UiError {
  return typeof e === 'object' && e !== null && 'kind' in e && 'message' in e;
}

export interface NewPair {
  name: string;
  localRoot: string;
  accountId: string;
  driveFolderId: string;
  remotePath: string;
  driveId: string | null;
  method: SyncMode;
  /** The vault the pair syncs through (its Drive folder is the vault's). */
  vaultId?: string | null;
  /** Leave out what .gitignore files leave out of git. */
  gitignore?: boolean;
  /** Preset ids: node, python, rust, android-gradle, flutter, docker, editors, os-junk. */
  presets?: string[];
  /** Sync hidden files and folders (names starting with a dot) too. */
  hidden?: boolean;
}

/** A new vault in a new or empty Drive folder. The passphrase goes straight to the call; nothing keeps it. */
export interface NewVault {
  accountId: string;
  folderId: string;
  driveId: string | null;
  remotePath: string;
  name: string;
  passphrase: string;
}

export interface NewView {
  name: string;
  localPath: string;
  accountId: string;
  driveFolderId: string;
  remotePath: string;
  driveId: string | null;
  /** Bytes of unpinned content to keep; null is the daemon's default (20 GiB). */
  cacheLimit: number | null;
}

/** The notifications' words (#71); src-tauri/src/notify.rs fills in `{name}`, `{device}` and `{count}`. */
export interface NotifyText {
  conflictTitle: string;
  conflictBody: string;
  failedTitle: string;
  failedBody: string;
  authTitle: string;
  authBody: string;
  deletionTitle: string;
  deletionBody: string;
  offerTitle: string;
  offerBody: string;
  pairingTitle: string;
  pairingBody: string;
  summaryTitle: string;
  summaryBody: string;
}

/** The tray's words (tooltip and right-click menu) and the notifications', in the window's language. */
export interface TrayText {
  notify: NotifyText;
  tooltip: string;
  open: string;
  pauseAll: string;
  syncNow: string;
  settings: string;
  quit: string;
}

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),
  daemonState: () => invoke<Conn>('daemon_state'),
  startDaemon: () => invoke<void>('start_daemon'),
  setTrayText: (text: TrayText) => invoke<void>('set_tray_text', { text }),
  // The tray's popover and flyout (#69).
  trayHide: () => invoke<void>('tray_hide'),
  trayFit: (height: number) => invoke<void>('tray_fit', { height }),
  trayOpen: (route: string | null) => invoke<void>('tray_open', { route }),
  /** The tray's menu under the popover's More button: its corner in logical pixels from the window's top-left. */
  trayMenu: (x: number, y: number) => invoke<void>('tray_menu', { x, y }),
  setTrayVisible: (visible: boolean) => invoke<void>('set_tray_visible', { visible }),
  // Folder drop (#71): whether a dropped path is a folder; the folder dropped on the Dock icon (macOS), once.
  isFolder: (path: string) => invoke<boolean>('is_folder', { path }),
  takeOpenedFolder: () => invoke<string | null>('take_opened_folder'),
  setStartAtLogin: (enabled: boolean) => invoke<void>('set_start_at_login', { enabled }),
  /** macOS: open or focus the Settings window (#68). */
  openSettings: () => invoke<void>('open_settings'),
  /** The OS accent where CSS can't read it (Windows), else null. */
  systemAccent: () => invoke<string | null>('system_accent'),

  status: () => invoke<DaemonStatus>('status'),
  createPair: (pair: NewPair) => invoke<string>('create_pair', { pair }),
  /** Renames a pair, changes its method or moves its folders (#65, #103). */
  updatePair: (id: string, edit: PairEdit) => invoke<void>('update_pair', { id, edit }),
  deletePair: (id: string) => invoke<void>('delete_pair', { id }),
  setPaused: (id: string, paused: boolean) => invoke<void>('set_paused', { id, paused }),
  /** Shows a pair's folder in Finder or File Explorer. */
  showPairFolder: (id: string) => invoke<void>('show_pair_folder', { id }),
  /** The update check (#196): the kept answer while under a day old, unless `now`; never throws for a failed check. */
  checkUpdate: (lang: string, now: boolean) => invoke<UpdateCheck>('check_update', { lang, now }),
  /** The download page the last check found, in the browser; the address stays on the Rust side. */
  openDownload: (lang: string) => invoke<void>('open_download', { lang }),
  /** The privacy policy or the terms on the Wasl site, in the browser (#146). */
  openSitePage: (page: 'privacy' | 'terms', lang: string) => invoke<void>('open_site_page', { page, lang }),
  preview: (id: string) => invoke<Preview>('preview', { id }),
  sync: (id: string, trigger: Trigger) => invoke<RunOutcome>('sync', { id, trigger }),
  syncAll: () => invoke<RunOutcome[]>('sync_all'),
  cancel: (id: string) => invoke<void>('cancel', { id }),
  approveOnce: (id: string) => invoke<void>('approve_once', { id }),
  blockedPlan: (id: string) => invoke<unknown>('blocked_plan', { id }),
  /** Every file a stopped plan would delete (slow: it compares both sides again). */
  deletionReview: (pairId: string) => invoke<DeletionReview>('deletion_review', { pairId }),
  /** Puts back what a stopped plan would delete and clears the stop; returns how many files came back. */
  restoreDeletions: (pairId: string) => invoke<number>('restore_deletions', { pairId }),

  runs: (pairId: string | null, limit: number) => invoke<RunRecord[]>('runs', { pairId, limit }),
  runEvents: (runId: string) => invoke<RunEvent[]>('run_events', { runId }),
  openConflicts: () => invoke<ConflictRecord[]>('open_conflicts'),
  conflictDetails: (id: string) => invoke<ConflictDetails | null>('conflict_details', { id }),
  resolveConflict: (id: string, choice: ConflictChoice) => invoke<ConflictUndo>('resolve_conflict', { id, choice }),
  undoConflict: (undo: ConflictUndo) => invoke<void>('undo_conflict', { undo }),

  agentChangesets: (all: boolean) => invoke<Changeset[]>('agent_changesets', { all }),
  agentChangeset: (id: string) => invoke<ChangesetDetails>('agent_changeset', { id }),
  agentDiff: (id: string, rel: string) => invoke<string>('agent_diff', { id, rel }),
  agentAccept: (id: string) => invoke<AcceptConfirmation>('agent_accept', { id }),
  agentRevert: (id: string) => invoke<RevertReport>('agent_revert', { id }),
  agentEnd: (id: string) => invoke<Changeset | null>('agent_end', { id }),
  agentSetup: () => invoke<AgentSetup>('agent_setup'),
  binEntries: () => invoke<BinEntry[]>('bin_entries'),
  restoreBin: (ids: string[]) => invoke<RestoreResult>('restore_bin', { ids }),
  /** Undo for the last restore: the ids restoreBin returned; the Rust side keeps the entries themselves. */
  undoBinRestore: (ids: string[]) => invoke<void>('undo_bin_restore', { ids }),
  deleteBinForever: (ids: string[]) => invoke<number>('delete_bin_forever', { ids }),
  emptyBin: () => invoke<number>('empty_bin'),

  accounts: () => invoke<Account[]>('accounts'),
  addAccount: () => invoke<string>('add_account'),
  removeAccount: (id: string) => invoke<void>('remove_account', { id }),
  driveFolders: (account: string, parentId: string) => invoke<DriveFolder[]>('drive_folders', { account, parentId }),
  resolveDrivePath: (account: string, path: string, create: boolean) =>
    invoke<DriveFolder>('resolve_drive_path', { account, path, create }),

  devices: () => invoke<DevicesInfo>('devices'),
  removeDevice: (id: string) => invoke<void>('remove_device', { id }),
  /** Returns the ID of the device reached. */
  connectDevice: (address: string) => invoke<string>('connect_device', { address }),
  /** Shares a pair with this person's devices under its existing code (or a new one), or under `share` (a code from another device). Returns the code. */
  sharePair: (id: string, share: string | null) => invoke<string>('share_pair', { id, share }),
  /** The pair's share code, or null when it doesn't sync with devices. */
  shareCode: (id: string) => invoke<string | null>('share_code', { id }),
  unsharePair: (id: string) => invoke<void>('unshare_pair', { id }),
  /** Who the pair is shared with, or null when it doesn't sync with devices. */
  sharing: (id: string) => invoke<PairSharing | null>('sharing', { id }),
  /** Shares the pair with every device known here (`all`) and/or the devices listed, each offered it. */
  shareWith: (id: string, all: boolean, devices: string[]) => invoke<void>('share_with', { id, all, devices }),
  offers: () => invoke<OfferInfo[]>('offers'),
  declineOffer: (share: string, from: string) => invoke<void>('decline_offer', { share, from }),
  /** Makes a pair from the offer in `localRoot`; it waits for its first-sync preview. */
  acceptOffer: (share: string, from: string, localRoot: string, name: string | null) =>
    invoke<Pair>('accept_offer', { share, from, localRoot, name }),
  keepDevice: (id: string) => invoke<void>('keep_device', { id }),
  inviteDevice: () => invoke<PairingStatus>('invite_device'),
  joinDevice: (code: string) => invoke<void>('join_device', { code }),
  pairingStatus: () => invoke<PairingStatus>('pairing_status'),
  confirmPairing: (accept: boolean) => invoke<PairingStatus>('confirm_pairing', { accept }),
  cancelPairing: () => invoke<void>('cancel_pairing'),

  views: () => invoke<ViewsInfo>('views'),
  addView: (view: NewView) => invoke<View>('add_view', { view }),
  /** Pinned files stay as ordinary files; with keepFiles every file is downloaded and kept. */
  removeView: (id: string, keepFiles: boolean) => invoke<void>('remove_view', { id, keepFiles }),
  setViewEnabled: (id: string, enabled: boolean) => invoke<void>('set_view_enabled', { id, enabled }),
  setViewCacheLimit: (id: string, limit: number) => invoke<void>('set_view_cache_limit', { id, limit }),
  refreshView: (id: string) => invoke<void>('refresh_view', { id }),
  /** A full path inside a view's folder: a file, a folder, or the view's folder itself. */
  pinPath: (path: string) => invoke<void>('pin_path', { path }),
  unpinPath: (path: string) => invoke<void>('unpin_path', { path }),

  vaults: () => invoke<VaultInfo[]>('vaults'),
  /** Slow on purpose (the key derivation): a few seconds. */
  createVault: (vault: NewVault) => invoke<VaultInfo>('create_vault', { vault }),
  /** A wrong passphrase is an `invalid` error. Slow on purpose. */
  unlockVault: (id: string, passphrase: string) => invoke<VaultInfo>('unlock_vault', { id, passphrase }),
  lockVault: (id: string) => invoke<void>('lock_vault', { id }),

  setting: (key: string) => invoke<string | null>('setting', { key }),
  setSetting: (key: string, value: string) => invoke<void>('set_setting', { key, value }),

  // Window chrome and the menu bar (#61).
  /** The macOS menu bar's words and the actions the window handles; nothing happens elsewhere. */
  setMenuText: (text: MenuText) => invoke<void>('set_menu_text', { text }),
  /** Where the traffic lights are (macOS); null on Windows and Linux. */
  windowChrome: () => invoke<Chrome | null>('window_chrome'),
  /** A double-click on the toolbar strip: zoom, minimise or nothing, as System Settings says (macOS). */
  titleBarDoubleClick: () => invoke<void>('title_bar_double_click'),
};
