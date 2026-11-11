import { Icon, StatusChip, Button, cx } from './core.jsx';

export const FILE_ICON = { image: 'image', video: 'movie', audio: 'music_note', pdf: 'picture_as_pdf', doc: 'description', text: 'article', folder: 'folder', shared: 'folder_shared', zip: 'folder_zip' };

/* ---------- ListItem: the one row used by pickers, bin, settings lists ---------- */
export function ListItem({ icon, iconFill, leading, thumb, headline, supporting, mono, trailing, meta, disabled, disabledReason, selected, chevron, onClick, tone }) {
  return (
    <div className={cx('ds-li', disabled && 'is-disabled', selected && 'is-selected', (onClick || chevron) && !disabled && 'is-action', tone && 'tone-' + tone)}
      aria-disabled={disabled || undefined}>
      {leading || (icon ? <span className={cx('ds-li-lead', thumb && 'is-thumb')}><Icon name={icon} fill={iconFill} size={24} /></span> : null)}
      <span className="ds-li-text">
        <span className="ds-li-headline">{headline}</span>
        {supporting ? <span className={cx('ds-li-supporting', mono && 'ds-mono')}>{supporting}</span> : null}
        {disabled && disabledReason ? <span className="ds-li-reason"><Icon name="block" size={14} />{disabledReason}</span> : null}
      </span>
      {meta ? <span className="ds-li-meta">{meta}</span> : null}
      {trailing}
      {chevron && !disabled ? <Icon name="chevron_right" size={24} className="ds-li-chev" /> : null}
    </div>
  );
}

/* ---------- ChangeRow: one planned action in a dry run ---------- */
export const ACTIONS = {
  upload: { icon: 'arrow_upward', label: 'To Drive' },
  download: { icon: 'arrow_downward', label: 'To phone' },
  bin: { icon: 'delete', label: 'To recycle bin' },
  conflict: { icon: 'call_split', label: 'Conflict' },
  skip: { icon: 'do_not_disturb_on', label: 'Skipped' },
  rename: { icon: 'drive_file_move', label: 'Move' },
};
export function ChangeRow({ action = 'upload', kind = 'image', name, path, size, reason }) {
  const a = ACTIONS[action];
  return (
    <div className={cx('ds-change', 'is-' + action)}>
      <span className="ds-change-file"><Icon name={FILE_ICON[kind] || 'description'} size={20} /></span>
      <span className="ds-change-text">
        <span className="ds-change-name">{name}</span>
        <span className="ds-change-path">{reason ? reason + ' · ' : ''}<span className="ds-mono">{path}</span></span>
      </span>
      <span className="ds-change-side">
        <span className="ds-change-action"><Icon name={a.icon} size={16} />{a.label}</span>
        {size ? <span className="ds-change-size">{size}</span> : null}
      </span>
    </div>
  );
}

/* ---------- ChangeSummary: counts before anything happens ---------- */
export function ChangeSummary({ unchanged, items }) {
  return (
    <div className="ds-summary">
      <div className="ds-summary-hero">
        <span className="ds-summary-num">{unchanged.toLocaleString('en-US')}</span>
        <span className="ds-summary-cap">files already match and will not be touched</span>
      </div>
      <div className="ds-summary-grid">
        {items.map((it) => (
          <div key={it.action} className={cx('ds-summary-tile', 'is-' + it.action, it.count === 0 && 'is-zero')}>
            <span className="ds-summary-tile-icon"><Icon name={ACTIONS[it.action].icon} size={18} /></span>
            <span className="ds-summary-tile-count">{it.count.toLocaleString('en-US')}</span>
            <span className="ds-summary-tile-label">{it.label || ACTIONS[it.action].label}</span>
            {it.size ? <span className="ds-summary-tile-size">{it.size}</span> : null}
          </div>
        ))}
      </div>
    </div>
  );
}

/* ---------- ConflictCard: both copies side by side, three safe choices ---------- */
export function ConflictCard({ name, kind = 'doc', path, phone, drive, choice, actions = true }) {
  const Side = ({ which, v }) => (
    <div className={cx('ds-conflict-side', choice === which && 'is-chosen')}>
      <span className="ds-conflict-over"><Icon name={which === 'drive' ? 'cloud' : 'mobile'} size={16} />{which === 'drive' ? 'On Drive' : 'On phone'}</span>
      <span className="ds-conflict-when">{v.when}</span>
      <span className="ds-conflict-meta">{v.meta}</span>
    </div>
  );
  return (
    <article className="ds-conflict">
      <header className="ds-conflict-head">
        <span className="ds-conflict-file"><Icon name={FILE_ICON[kind]} size={22} /></span>
        <span className="ds-conflict-title">
          <span className="ds-conflict-name">{name}</span>
          <span className="ds-conflict-path ds-mono">{path}</span>
        </span>
      </header>
      <div className="ds-conflict-chip"><StatusChip status="conflict" size="sm" label="Changed on both sides" /></div>
      
      <div className="ds-conflict-sides"><Side which="phone" v={phone} /><Side which="drive" v={drive} /></div>
      {actions ? (
        <>
          <div className="ds-conflict-actions">
            <Button variant="tonal" size="sm">Keep both</Button>
            <Button variant="outlined" size="sm">Keep phone</Button>
            <Button variant="outlined" size="sm">Keep Drive</Button>
          </div>
          <p className="ds-conflict-note">The copy you don't keep goes to the recycle bin for 30 days.</p>
        </>
      ) : null}
    </article>
  );
}

/* ---------- VersionItem: one entry in a file's history ---------- */
export function VersionItem({ when, source = 'phone', size, note, current, first, last }) {
  const src = { phone: ['mobile', 'Edited on phone'], drive: ['cloud', 'Edited on Drive'], conflict: ['call_split', 'Conflict copy'], restored: ['history', 'Restored'] }[source];
  return (
    <div className={cx('ds-version', current && 'is-current', first && 'is-first', last && 'is-last')}>
      <span className="ds-version-rail" aria-hidden="true"><span className="ds-version-dot" /></span>
      <div className="ds-version-body">
        <div className="ds-version-head">
          <span className="ds-version-when">{when}</span>
          {current ? <span className="ds-tag">Current</span> : null}
        </div>
        <div className="ds-version-meta"><Icon name={src[0]} size={16} />{src[1]} · {size}{note ? ' · ' + note : ''}</div>
        {!current ? (
          <div className="ds-version-actions">
            <Button variant="text" size="sm" icon="history">Restore</Button>
            <Button variant="text" size="sm" icon="content_copy">Save as copy</Button>
          </div>
        ) : null}
      </div>
    </div>
  );
}

/* ---------- Diagnostics ---------- */
export function HealthCheckRow({ state = 'ok', title, detail, action }) {
  const map = { ok: ['check_circle', 'synced', 'OK'], warn: ['warning', 'approval', 'Check'], fail: ['error', 'failed', 'Problem'], wait: ['schedule', 'waiting', 'Waiting'] };
  const [icon, tone, word] = map[state];
  return (
    <div className={cx('ds-health', 'is-' + state)}>
      <span className={cx('ds-health-icon', 'tone-' + tone)}><Icon name={icon} fill size={22} title={word} /></span>
      <span className="ds-health-text">
        <span className="ds-health-title">{title}</span>
        <span className="ds-health-detail">{detail}</span>
      </span>
      {action ? <Button variant="tonal" size="sm">{action}</Button> : null}
    </div>
  );
}

export function LogView({ lines }) {
  return (
    <div className="ds-log" role="log">
      {lines.map((l, i) => (
        <div key={i} className={cx('ds-log-line', 'lv-' + (l[1] || 'info').toLowerCase())}>
          <span className="t">{l[0]}</span><span className="lv">{l[1]}</span><span className="m">{l[2]}</span>
        </div>
      ))}
    </div>
  );
}
