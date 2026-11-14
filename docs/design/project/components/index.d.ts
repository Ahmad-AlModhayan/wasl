import type * as React from 'react';

export type SyncStatus = 'synced' | 'syncing' | 'waiting' | 'approval' | 'conflict' | 'failed';
export type SyncMode = 'two' | 'up' | 'down';
export type Side = 'phone' | 'drive';
export type ChangeAction = 'upload' | 'download' | 'bin' | 'conflict' | 'skip' | 'rename';
export type FileKind = 'image' | 'video' | 'audio' | 'pdf' | 'doc' | 'text' | 'folder' | 'shared' | 'zip';

/** Material Symbols Rounded, weight 400. */
export interface IconProps { name: string; fill?: boolean; size?: number; title?: string; className?: string; style?: React.CSSProperties }
export declare function Icon(props: IconProps): React.ReactElement;

/** The mark, lockup, adaptive icon, themed icon or notification glyph. */
export interface LogoProps { variant?: 'mark' | 'lockup' | 'app-icon' | 'themed-icon' | 'notification'; size?: number; mask?: 'circle' | 'squircle' | 'rounded' | 'full'; color?: string; title?: string }
export declare function Logo(props: LogoProps): React.ReactElement;

/** Icon + word for one of the six states. Quiet states plain, loud states tonal. */
export interface StatusChipProps { status?: SyncStatus; label?: string; progress?: number; count?: number; variant?: 'tonal' | 'plain' | 'icon'; size?: 'md' | 'sm' }
export declare function StatusChip(props: StatusChipProps): React.ReactElement;
export interface StatusIconProps { status: SyncStatus; size?: number; title?: string }
export declare function StatusIcon(props: StatusIconProps): React.ReactElement;

/** One per screen, for the most urgent state. */
export interface StatusBannerProps { status?: SyncStatus; title?: React.ReactNode; children?: React.ReactNode; actions?: React.ReactNode }
export declare function StatusBanner(props: StatusBannerProps): React.ReactElement;

export interface LinearProgressProps { value?: number }
export declare function LinearProgress(props: LinearProgressProps): React.ReactElement;
export interface SyncProgressProps { phase?: 'transfer' | 'scanning'; value?: number; done?: number; total?: number; doneBytes?: string; totalBytes?: string; direction?: 'up' | 'down' | 'both'; eta?: string; current?: string; onPause?: false | (() => void) }
export declare function SyncProgress(props: SyncProgressProps): React.ReactElement;

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> { variant?: 'filled' | 'tonal' | 'outlined' | 'text' | 'danger' | 'danger-text'; size?: 'sm' | 'md' | 'lg'; icon?: string }
export declare function Button(props: ButtonProps): React.ReactElement;
export interface IconButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> { icon: string; label: string; variant?: 'standard' | 'tonal'; fill?: boolean; badge?: React.ReactNode }
export declare function IconButton(props: IconButtonProps): React.ReactElement;
export interface FabProps extends React.ButtonHTMLAttributes<HTMLButtonElement> { icon?: string }
export declare function Fab(props: FabProps): React.ReactElement;
export interface ChipProps { selected?: boolean; icon?: string; children?: React.ReactNode; trailing?: React.ReactNode; onClick?: () => void }
export declare function Chip(props: ChipProps): React.ReactElement;

/** Sizes in GB. */
export interface StorageBarProps { side?: Side; total: number; pair: number; other: number; compact?: boolean; label?: string }
export declare function StorageBar(props: StorageBarProps): React.ReactElement;
export interface PathRowProps { side?: Side; path?: string; meta?: string; onPick?: boolean | (() => void); error?: string; dense?: boolean }
export declare function PathRow(props: PathRowProps): React.ReactElement;
export interface FolderPairCardProps { name: string; mode?: SyncMode; status?: SyncStatus; progress?: number; count?: number; phonePath: string; drivePath: string; lastSync?: string; conditions?: string; storage?: StorageBarProps[]; sync?: SyncProgressProps; children?: React.ReactNode }
export declare function FolderPairCard(props: FolderPairCardProps): React.ReactElement;
export interface SyncModeOptionProps { mode?: SyncMode; selected?: boolean; description: string; recommended?: boolean }
export declare function SyncModeOption(props: SyncModeOptionProps): React.ReactElement;
export interface CalloutProps { tone?: 'info' | 'warning' | 'danger' | 'success'; title?: React.ReactNode; children?: React.ReactNode; action?: React.ReactNode }
export declare function Callout(props: CalloutProps): React.ReactElement;

export interface ListItemProps { icon?: string; iconFill?: boolean; leading?: React.ReactNode; thumb?: boolean; headline: React.ReactNode; supporting?: React.ReactNode; mono?: boolean; meta?: React.ReactNode; trailing?: React.ReactNode; chevron?: boolean; selected?: boolean; disabled?: boolean; disabledReason?: string; tone?: 'folder' | 'drive'; onClick?: () => void }
export declare function ListItem(props: ListItemProps): React.ReactElement;
export interface ChangeRowProps { action?: ChangeAction; kind?: FileKind; name: string; path: string; size?: string; reason?: string }
export declare function ChangeRow(props: ChangeRowProps): React.ReactElement;
export interface ChangeSummaryProps { unchanged: number; items: { action: ChangeAction; count: number; size?: string; label?: string }[] }
export declare function ChangeSummary(props: ChangeSummaryProps): React.ReactElement;
export interface ConflictCardProps { name: string; kind?: FileKind; path: string; phone: { when: string; meta: string }; drive: { when: string; meta: string }; choice?: Side; actions?: boolean }
export declare function ConflictCard(props: ConflictCardProps): React.ReactElement;
export interface VersionItemProps { when: string; source?: 'phone' | 'drive' | 'conflict' | 'restored'; size: string; note?: string; current?: boolean; first?: boolean; last?: boolean }
export declare function VersionItem(props: VersionItemProps): React.ReactElement;
export interface HealthCheckRowProps { state?: 'ok' | 'warn' | 'fail' | 'wait'; title: string; detail: string; action?: string }
export declare function HealthCheckRow(props: HealthCheckRowProps): React.ReactElement;
export interface LogViewProps { lines: [time: string, level: 'OK' | 'INFO' | 'WARN' | 'ERROR', message: string][] }
export declare function LogView(props: LogViewProps): React.ReactElement;

export interface TopAppBarProps { title: React.ReactNode; subtitle?: React.ReactNode; nav?: 'back' | 'close' | null; actions?: React.ReactNode; large?: boolean; scrolled?: boolean; logo?: boolean }
export declare function TopAppBar(props: TopAppBarProps): React.ReactElement;
export interface NavigationBarProps { active?: 'pairs' | 'problems' | 'bin' | 'settings'; badges?: Partial<Record<'pairs' | 'problems' | 'bin' | 'settings', React.ReactNode>> }
export declare function NavigationBar(props: NavigationBarProps): React.ReactElement;
export interface PathBreadcrumbProps { root?: string; rootIcon?: string; parts?: string[] }
export declare function PathBreadcrumb(props: PathBreadcrumbProps): React.ReactElement;
export interface SegmentedButtonProps { items: string[]; value: string }
export declare function SegmentedButton(props: SegmentedButtonProps): React.ReactElement;
export interface SwitchProps { on?: boolean }
export declare function Switch(props: SwitchProps): React.ReactElement;
export interface SettingRowProps { icon?: string; title: React.ReactNode; summary?: React.ReactNode; control?: 'switch' | 'value' | 'chevron' | null; on?: boolean; value?: React.ReactNode; danger?: boolean; disabled?: boolean }
export declare function SettingRow(props: SettingRowProps): React.ReactElement;
export interface SectionHeaderProps { children?: React.ReactNode; action?: React.ReactNode }
export declare function SectionHeader(props: SectionHeaderProps): React.ReactElement;
export interface TextFieldProps { label: string; value?: string; helper?: string; error?: string; mono?: boolean; leading?: string; trailing?: string }
export declare function TextField(props: TextFieldProps): React.ReactElement;
export interface DialogProps { icon?: string; title: React.ReactNode; children?: React.ReactNode; confirm?: string; cancel?: string; danger?: boolean }
export declare function Dialog(props: DialogProps): React.ReactElement;
export interface SnackbarProps { children?: React.ReactNode; action?: string | null }
export declare function Snackbar(props: SnackbarProps): React.ReactElement;
export interface PermissionStepProps { icon: string; title: string; why: string; state?: 'needed' | 'next' | 'granted'; optional?: boolean; cta?: string }
export declare function PermissionStep(props: PermissionStepProps): React.ReactElement;

/** Showcase only. */
export interface PhoneFrameProps { children?: React.ReactNode; time?: string; notif?: boolean; label?: string; width?: number; height?: number; overlay?: React.ReactNode }
export declare function PhoneFrame(props: PhoneFrameProps): React.ReactElement;
export interface ScreenProps { appbar?: React.ReactNode; children?: React.ReactNode; bottom?: React.ReactNode; bottomPlain?: boolean; fab?: React.ReactNode; nav?: React.ReactNode; overlay?: React.ReactNode }
export declare function Screen(props: ScreenProps): React.ReactElement;
export interface ShowcaseProps { title: string; intro?: string; notes?: [string, string][]; children?: React.ReactNode; narrow?: boolean }
export declare function Showcase(props: ShowcaseProps): React.ReactElement;

declare global {
  interface Window {
    DriveSync: {
      Icon: typeof Icon; Logo: typeof Logo; StatusChip: typeof StatusChip; StatusIcon: typeof StatusIcon; StatusBanner: typeof StatusBanner;
      LinearProgress: typeof LinearProgress; SyncProgress: typeof SyncProgress; Button: typeof Button; IconButton: typeof IconButton; Fab: typeof Fab; Chip: typeof Chip;
      StorageBar: typeof StorageBar; PathRow: typeof PathRow; FolderPairCard: typeof FolderPairCard; SyncModeOption: typeof SyncModeOption; Callout: typeof Callout;
      ListItem: typeof ListItem; ChangeRow: typeof ChangeRow; ChangeSummary: typeof ChangeSummary; ConflictCard: typeof ConflictCard; VersionItem: typeof VersionItem;
      HealthCheckRow: typeof HealthCheckRow; LogView: typeof LogView; TopAppBar: typeof TopAppBar; NavigationBar: typeof NavigationBar; PathBreadcrumb: typeof PathBreadcrumb;
      SegmentedButton: typeof SegmentedButton; Switch: typeof Switch; SettingRow: typeof SettingRow; SectionHeader: typeof SectionHeader; TextField: typeof TextField;
      Dialog: typeof Dialog; Snackbar: typeof Snackbar; PermissionStep: typeof PermissionStep; PhoneFrame: typeof PhoneFrame; Screen: typeof Screen; Showcase: typeof Showcase;
    };
  }
}
