import { Icon, StatusChip, SyncProgress, cx } from './core.jsx';

const fmt = (gb) => (gb >= 100 ? Math.round(gb) : gb >= 10 ? gb.toFixed(1).replace(/\.0$/, '') : gb.toFixed(1)) + ' GB';

/* ---------- StorageBar: this pair / other / free, segments split by a 2px gap ---------- */
export function StorageBar({ side = 'drive', total, pair, other, compact = false, label }) {
  const free = Math.max(0, total - pair - other);
  const low = free / total < 0.1;
  const pct = (v) => `${(v / total) * 100}%`;
  const name = label || (side === 'drive' ? 'Drive' : 'Phone');
  return (
    <div className={cx('ds-storage', compact && 'is-compact', low && 'is-low')}>
      <div className="ds-storage-head">
        <span className="ds-storage-side">
          <Icon name={side === 'drive' ? 'cloud' : 'mobile'} size={16} />{name}
        </span>
        <span className="ds-storage-total">
          {low ? <strong className="ds-storage-lowtag"><Icon name="warning" fill size={14} />Low space · </strong> : null}
          {fmt(free)} free of {fmt(total)}
        </span>
      </div>
      <div className="ds-storage-bar" role="img" aria-label={`${name}: this pair ${fmt(pair)}, other ${fmt(other)}, free ${fmt(free)} of ${fmt(total)}`}>
        <span className="seg seg-pair" style={{ width: pct(pair) }} />
        <span className="seg seg-other" style={{ width: pct(other) }} />
      </div>
      {!compact ? (
        <div className="ds-storage-legend">
          <span><i className="key key-pair" />This pair {fmt(pair)}</span>
          <span><i className="key key-other" />Other {fmt(other)}</span>
          <span><i className="key key-free" />Free {fmt(free)}</span>
        </div>
      ) : null}
    </div>
  );
}

/* ---------- PathRow: one side of a pair ---------- */
export function PathRow({ side = 'phone', path, meta, onPick, error, dense }) {
  return (
    <div className={cx('ds-pathrow', 'is-' + side, error && 'has-error', dense && 'is-dense', onPick && 'is-action')}>
      <span className="ds-pathrow-icon"><Icon name={side === 'drive' ? 'cloud' : 'mobile'} size={20} /></span>
      <span className="ds-pathrow-text">
        <span className="ds-pathrow-over">{side === 'drive' ? 'Drive' : 'Phone'}</span>
        <span className="ds-pathrow-path">{path || <em>Choose a folder</em>}</span>
        {meta ? <span className="ds-pathrow-meta">{meta}</span> : null}
        {error ? <span className="ds-pathrow-error"><Icon name="error" fill size={14} />{error}</span> : null}
      </span>
      {onPick ? <Icon name="chevron_right" size={24} className="ds-pathrow-chev" /> : null}
    </div>
  );
}

export const MODES = {
  two: { title: 'Two-way', icon: 'sync_alt', arrow: 'swap_horiz', short: 'Two-way' },
  up: { title: 'Phone → Drive', icon: 'cloud_upload', arrow: 'arrow_forward', short: 'Phone → Drive' },
  down: { title: 'Drive → Phone', icon: 'cloud_download', arrow: 'arrow_back', short: 'Drive → Phone' },
};

/* ---------- FolderPairCard ---------- */
export function FolderPairCard({ name, mode = 'two', status = 'synced', progress, count, phonePath, drivePath, lastSync, conditions, storage, sync, children }) {
  return (
    <article className={cx('ds-paircard', 'is-' + status)}>
      <header className="ds-paircard-head">
        <div className="ds-paircard-title">
          <h3>{name}</h3>
          <p>{MODES[mode].short}{conditions ? ' · ' + conditions : ''}</p>
        </div>
        <button className="ds-iconbtn v-standard" type="button" aria-label={`More for ${name}`}><Icon name="more_vert" /></button>
      </header>
      <div className="ds-paircard-status">
        <StatusChip status={status} progress={sync ? undefined : progress} count={count} variant={['approval', 'conflict', 'failed'].includes(status) ? 'tonal' : 'plain'} />
        {lastSync ? <span className="ds-paircard-last">{lastSync}</span> : null}
      </div>
      {sync ? <SyncProgress {...sync} /> : null}
      <div className={cx('ds-paircard-paths', 'mode-' + mode)}>
        <PathRow side="phone" path={phonePath} dense />
        <span className="ds-paircard-dir" aria-label={MODES[mode].title}><Icon name={MODES[mode].arrow} size={18} /></span>
        <PathRow side="drive" path={drivePath} dense />
      </div>
      {storage ? <div className="ds-paircard-storage">{storage.map((s, i) => <StorageBar key={i} compact {...s} />)}</div> : null}
      {children}
    </article>
  );
}

/* ---------- SyncModeOption: radio card ---------- */
export function SyncModeOption({ mode = 'two', selected, description, recommended }) {
  const m = MODES[mode];
  return (
    <label className={cx('ds-mode', selected && 'is-selected')}>
      <input type="radio" name="sync-mode" defaultChecked={!!selected} className="ds-visually-hidden" />
      <span className="ds-radio" aria-hidden="true"><Icon name={selected ? 'radio_button_checked' : 'radio_button_unchecked'} size={24} /></span>
      <span className="ds-mode-body">
        <span className="ds-mode-title"><span className="ds-mode-name">{m.title}</span>{recommended ? <span className="ds-mode-rec">Recommended</span> : null}</span>
        <span className="ds-mode-desc">{description}</span>
      </span>
      <span className={cx('ds-mode-diagram', 'mode-' + mode)} aria-hidden="true">
        <Icon name="mobile" size={20} />
        <Icon name={m.arrow} size={18} className="ds-mode-arrow" />
        <Icon name="cloud" size={20} />
      </span>
    </label>
  );
}

/* ---------- Callout: inline guidance inside forms ---------- */
export function Callout({ tone = 'info', title, children, action }) {
  const icon = { info: 'info', warning: 'warning', danger: 'error', success: 'check_circle' }[tone];
  return (
    <div className={cx('ds-callout', 'tone-' + tone)} role={tone === 'danger' ? 'alert' : 'note'}>
      <Icon name={icon} fill size={20} className="ds-callout-icon" />
      <div className="ds-callout-body">
        {title ? <div className="ds-callout-title">{title}</div> : null}
        <div className="ds-callout-text">{children}</div>
        {action ? <div className="ds-callout-action">{action}</div> : null}
      </div>
    </div>
  );
}
