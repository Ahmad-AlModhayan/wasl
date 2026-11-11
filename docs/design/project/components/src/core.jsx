import { ICONS } from './icon-paths.js';
import { MARK } from './mark-paths.js';

export const cx = (...a) => a.filter(Boolean).join(' ');

/* ---------- Icon: Material Symbols Rounded, weight 400, optical size 24 ---------- */
export function Icon({ name, fill = false, size = 24, className, title, style }) {
  const d = (fill && ICONS[name + '_fill']) || ICONS[name] || ICONS.help;
  return (
    <svg className={cx('ds-icon', className)} width={size} height={size} viewBox="0 -960 960 960"
      aria-hidden={title ? undefined : 'true'} role={title ? 'img' : undefined} style={style}>
      {title ? <title>{title}</title> : null}
      <path d={d} fill="currentColor" />
    </svg>
  );
}

/* ---------- Brand mark ---------- */
function MarkPaths({ kind = 'mark', color = 'currentColor' }) {
  const m = MARK[kind];
  const [A, B, HA, HB] = m.paths;
  return (
    <g fill="none" stroke={color} strokeLinecap="round" strokeLinejoin="round">
      <path d={A} strokeWidth={m.w} />
      <path d={B} strokeWidth={m.w} />
      <path d={HA + ' ' + HB} fill={color} strokeWidth="0.9" />
    </g>
  );
}

/**
 * variant: 'mark' | 'lockup' | 'app-icon' | 'themed-icon' | 'notification'
 * mask (app-icon / themed-icon): 'circle' | 'squircle' | 'rounded' | 'full'
 */
export function Logo({ variant = 'mark', size = 48, mask = 'circle', color, className, title = 'Drive Sync' }) {
  if (variant === 'mark' || variant === 'notification') {
    return (
      <svg className={cx('ds-logo', className)} width={size} height={size} viewBox="0 0 24 24" role="img" aria-label={title}
        style={{ color: color || (variant === 'mark' ? 'var(--logo-ink)' : 'currentColor') }}>
        <MarkPaths kind={variant === 'mark' ? 'mark' : 'notif'} />
      </svg>
    );
  }
  if (variant === 'lockup') {
    const ms = 2.2, mbox = 24 * ms, wm = MARK.wordmark, base = mbox / 2 + wm.cap / 2, tx = mbox + 10, W = tx + wm.width + 2;
    return (
      <svg className={cx('ds-logo', className)} height={size} viewBox={`0 0 ${W} ${mbox}`} role="img" aria-label={title}>
        <g transform={`scale(${ms})`}><MarkPaths color={color || 'var(--logo-ink)'} /></g>
        <path transform={`translate(${tx} ${base})`} fill={color || 'var(--on-surface)'} d={wm.d} />
      </svg>
    );
  }
  // adaptive icon on the 108dp canvas, viewed through the 72dp window with a launcher mask
  const S = 2.15, T = 54 - 12 * S;
  const clip = {
    circle: <circle cx="54" cy="54" r="36" />,
    squircle: <path d="M54 18C82 18 90 26 90 54S82 90 54 90 18 82 18 54 26 18 54 18Z" />,
    rounded: <rect x="18" y="18" width="72" height="72" rx="16" />,
    full: <rect x="18" y="18" width="72" height="72" />,
  }[mask];
  const id = 'm' + React.useId().replace(/[^a-zA-Z0-9]/g, '');
  const themed = variant === 'themed-icon';
  return (
    <svg className={cx('ds-logo', className)} width={size} height={size} viewBox="18 18 72 72" role="img" aria-label={title}>
      <defs><clipPath id={id}>{clip}</clipPath></defs>
      <g clipPath={`url(#${id})`}>
        <rect width="108" height="108" fill={themed ? 'var(--secondary-container)' : 'var(--brand-green)'} />
        <g transform={`translate(${T} ${T}) scale(${S})`}>
          <MarkPaths color={themed ? 'var(--on-secondary-container)' : 'var(--brand-mint)'} />
        </g>
      </g>
    </svg>
  );
}

/* ---------- Status system ---------- */
export const STATUS = {
  synced: { icon: 'cloud_done', fill: true, label: 'Synced' },
  syncing: { icon: 'sync', fill: false, label: 'Syncing' },
  waiting: { icon: 'wifi', fill: false, label: 'Waiting for Wi‑Fi' },
  approval: { icon: 'front_hand', fill: true, label: 'Needs approval' },
  conflict: { icon: 'call_split', fill: false, label: 'Conflict' },
  failed: { icon: 'error', fill: true, label: 'Failed' },
};

export function StatusIcon({ status, size = 20, title }) {
  const s = STATUS[status];
  return (
    <span className={cx('ds-status-icon', 'is-' + status)} title={title}>
      <Icon name={s.icon} fill={s.fill} size={size} title={title} />
    </span>
  );
}

/** variant: 'tonal' (default chip) | 'plain' (icon + text, no ground) | 'icon' (icon only, labelled) */
export function StatusChip({ status = 'synced', label, progress, count, variant = 'tonal', size = 'md' }) {
  const s = STATUS[status];
  let text = label || s.label;
  if (!label && status === 'syncing' && typeof progress === 'number') text = `Syncing ${Math.round(progress * 100)}%`;
  if (!label && status === 'conflict' && count > 1) text = `${count} conflicts`;
  if (!label && status === 'failed' && count > 1) text = `${count} failed`;
  if (variant === 'icon') return <StatusIcon status={status} size={size === 'sm' ? 18 : 20} title={text} />;
  return (
    <span className={cx('ds-status', 'is-' + status, 'v-' + variant, 's-' + size)} role="status">
      <Icon name={s.icon} fill={s.fill} size={size === 'sm' ? 16 : 18} className="ds-status-glyph" />
      <span className="ds-status-text">{text}</span>
    </span>
  );
}

export function StatusBanner({ status = 'approval', title, children, actions }) {
  const s = STATUS[status];
  return (
    <section className={cx('ds-banner', 'is-' + status)} role={status === 'failed' ? 'alert' : 'status'}>
      <Icon name={s.icon} fill={s.fill} size={24} className="ds-banner-icon" />
      <div className="ds-banner-body">
        {title ? <div className="ds-banner-title">{title}</div> : null}
        {children ? <div className="ds-banner-text">{children}</div> : null}
        {actions ? <div className="ds-banner-actions">{actions}</div> : null}
      </div>
    </section>
  );
}

/* ---------- Progress ---------- */
export function LinearProgress({ value, tone = 'syncing' }) {
  const ind = typeof value !== 'number';
  return (
    <div className={cx('ds-progress', ind && 'is-indeterminate', 'tone-' + tone)} role="progressbar"
      aria-valuemin={0} aria-valuemax={100} aria-valuenow={ind ? undefined : Math.round(value * 100)}>
      <div className="ds-progress-fill" style={ind ? undefined : { width: `${Math.max(2, value * 100)}%` }} />
      {!ind ? <div className="ds-progress-stop" /> : null}
    </div>
  );
}

export function SyncProgress({ phase = 'transfer', value, done, total, doneBytes, totalBytes, direction = 'both', eta, current, onPause }) {
  const scanning = phase === 'scanning';
  const verb = direction === 'up' ? 'Uploading' : direction === 'down' ? 'Downloading' : 'Syncing';
  return (
    <div className="ds-syncprog">
      <div className="ds-syncprog-head">
        <span className="ds-syncprog-title">
          {scanning ? `Comparing folders… ${total ? total.toLocaleString('en-US') + ' files checked' : ''}` : `${verb} ${done.toLocaleString('en-US')} of ${total.toLocaleString('en-US')}`}
        </span>
        {!scanning && totalBytes ? <span className="ds-syncprog-bytes">{doneBytes} of {totalBytes}</span> : null}
      </div>
      <LinearProgress value={scanning ? undefined : value} />
      <div className="ds-syncprog-foot">
        <span className="ds-syncprog-meta">{current ? <span className="ds-mono">{current}</span> : null}{current && eta ? ' · ' : ''}{eta}</span>
        {onPause !== false ? <button className="ds-btn v-text s-sm" type="button"><Icon name="pause" fill size={18} />Pause</button> : null}
      </div>
    </div>
  );
}

/* ---------- Buttons ---------- */
export function Button({ variant = 'filled', icon, children, size = 'md', className, disabled, ...rest }) {
  return (
    <button type="button" {...rest} disabled={disabled} className={cx('ds-btn', 'v-' + variant, 's-' + size, icon && 'has-icon', className)}>
      {icon ? <Icon name={icon} size={18} /> : null}
      {children}
    </button>
  );
}

export function IconButton({ icon, label, variant = 'standard', fill, badge, ...rest }) {
  return (
    <button type="button" {...rest} className={cx('ds-iconbtn', 'v-' + variant)} aria-label={label} title={label}>
      <Icon name={icon} fill={fill} size={24} />
      {badge ? <span className="ds-badge">{badge}</span> : null}
    </button>
  );
}

export function Fab({ icon = 'add', children, ...rest }) {
  return (
    <button type="button" {...rest} className={cx('ds-fab', children && 'is-extended')}>
      <Icon name={icon} size={24} />
      {children ? <span>{children}</span> : null}
    </button>
  );
}

export function Chip({ selected, icon, children, onClick, trailing }) {
  return (
    <button type="button" className={cx('ds-chip', selected && 'is-selected')} aria-pressed={!!selected} onClick={onClick}>
      {selected ? <Icon name="check" size={18} /> : icon ? <Icon name={icon} size={18} /> : null}
      <span>{children}</span>
      {trailing ? <span className="ds-chip-count">{trailing}</span> : null}
    </button>
  );
}
