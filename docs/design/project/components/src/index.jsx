import { Icon, Logo, STATUS, StatusIcon, StatusChip, StatusBanner, LinearProgress, SyncProgress, Button, IconButton, Fab, Chip } from './core.jsx';
import { StorageBar, PathRow, MODES, FolderPairCard, SyncModeOption, Callout } from './pairs.jsx';
import { ListItem, ChangeRow, ChangeSummary, ConflictCard, VersionItem, HealthCheckRow, LogView, ACTIONS, FILE_ICON } from './files.jsx';
import { TopAppBar, NavigationBar, PathBreadcrumb, SegmentedButton, Switch, SettingRow, SectionHeader, TextField, Dialog, Snackbar, PermissionStep, PhoneFrame, Screen, Showcase } from './shell.jsx';

const api = {
  Icon, Logo, StatusChip, StatusIcon, StatusBanner, LinearProgress, SyncProgress,
  Button, IconButton, Fab, Chip,
  StorageBar, PathRow, FolderPairCard, SyncModeOption, Callout,
  ListItem, ChangeRow, ChangeSummary, ConflictCard, VersionItem, HealthCheckRow, LogView,
  TopAppBar, NavigationBar, PathBreadcrumb, SegmentedButton, Switch, SettingRow, SectionHeader, TextField,
  Dialog, Snackbar, PermissionStep, PhoneFrame, Screen, Showcase,
  STATUS, MODES, ACTIONS, FILE_ICON,
};
window.DriveSync = Object.assign(window.DriveSync || {}, api);
