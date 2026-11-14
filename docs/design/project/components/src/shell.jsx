import { Icon, IconButton, Button, Logo, cx } from './core.jsx';

/* ---------- Navigation ---------- */
export function TopAppBar({ title, subtitle, nav = 'back', actions, large, scrolled, logo }) {
  return (
    <header className={cx('ds-appbar', large && 'is-large', scrolled && 'is-scrolled')}>
      <div className="ds-appbar-row">
        {nav === 'back' ? <IconButton icon="arrow_back" label="Back" /> : nav === 'close' ? <IconButton icon="close" label="Close" /> : null}
        {logo ? <span className="ds-appbar-logo"><Logo variant="mark" size={28} /></span> : null}
        {!large ? (
          <div className="ds-appbar-titles">
            <h1 className="ds-appbar-title">{title}</h1>
            {subtitle ? <p className="ds-appbar-sub">{subtitle}</p> : null}
          </div>
        ) : <span style={{ flex: 1 }} />}
        <div className="ds-appbar-actions">{actions}</div>
      </div>
      {large ? (
        <div className="ds-appbar-large">
          <h1 className="ds-appbar-title">{title}</h1>
          {subtitle ? <p className="ds-appbar-sub">{subtitle}</p> : null}
        </div>
      ) : null}
    </header>
  );
}

export const NAV_ITEMS = [
  { key: 'pairs', label: 'Pairs', icon: 'folder_copy' },
  { key: 'problems', label: 'Problems', icon: 'report' },
  { key: 'bin', label: 'Recycle bin', icon: 'delete' },
  { key: 'settings', label: 'Settings', icon: 'settings' },
];
export function NavigationBar({ active = 'pairs', badges = {} }) {
  return (
    <nav className="ds-navbar">
      {NAV_ITEMS.map((it) => (
        <a key={it.key} className={cx('ds-navbar-item', active === it.key && 'is-active')} aria-current={active === it.key ? 'page' : undefined}>
          <span className="ds-navbar-ind">
            <Icon name={it.icon} fill={active === it.key} size={24} />
            {badges[it.key] ? <span className="ds-badge">{badges[it.key]}</span> : null}
          </span>
          <span className="ds-navbar-label">{it.label}</span>
        </a>
      ))}
    </nav>
  );
}

export function PathBreadcrumb({ root = 'Phone', rootIcon = 'mobile', parts = [] }) {
  return (
    <nav className="ds-crumbs" aria-label="Folder path">
      <span className={cx('ds-crumb', 'is-root', parts.length === 0 && 'is-current')}><Icon name={rootIcon} size={18} />{root}</span>
      {parts.map((p, i) => (
        <React.Fragment key={i}>
          <Icon name="chevron_right" size={18} className="ds-crumb-sep" />
          <span className={cx('ds-crumb', i === parts.length - 1 && 'is-current')}>{p}</span>
        </React.Fragment>
      ))}
    </nav>
  );
}

export function SegmentedButton({ items, value }) {
  return (
    <div className="ds-seg" role="radiogroup">
      {items.map((it) => (
        <button key={it} type="button" role="radio" aria-checked={it === value} className={cx('ds-seg-item', it === value && 'is-selected')}>
          {it === value ? <Icon name="check" size={18} /> : null}{it}
        </button>
      ))}
    </div>
  );
}

/* ---------- Inputs ---------- */
export function Switch({ on }) {
  return (
    <span className={cx('ds-switch', on && 'is-on')} role="switch" aria-checked={!!on}>
      <span className="ds-switch-thumb">{on ? <Icon name="check" size={16} /> : null}</span>
    </span>
  );
}

export function SettingRow({ icon, title, summary, control = 'chevron', on, value, danger, disabled }) {
  return (
    <div className={cx('ds-setting', danger && 'is-danger', disabled && 'is-disabled')}>
      {icon ? <span className="ds-setting-icon"><Icon name={icon} size={24} /></span> : null}
      <span className="ds-setting-text">
        <span className="ds-setting-title">{title}</span>
        {summary ? <span className="ds-setting-summary">{summary}</span> : null}
      </span>
      {control === 'switch' ? <Switch on={on} /> : control === 'value' ? <span className="ds-setting-value">{value}</span> : control === 'chevron' ? <Icon name="chevron_right" size={24} className="ds-setting-chev" /> : null}
    </div>
  );
}

export function SectionHeader({ children, action }) {
  return <div className="ds-section-h"><span>{children}</span>{action}</div>;
}

export function TextField({ label, value, helper, error, mono, leading, trailing }) {
  return (
    <div className={cx('ds-field', error && 'has-error')}>
      <div className="ds-field-box">
        {leading ? <Icon name={leading} size={24} className="ds-field-lead" /> : null}
        <span className="ds-field-inner">
          <span className="ds-field-label">{label}</span>
          <span className={cx('ds-field-value', mono && 'ds-mono')}>{value}</span>
        </span>
        {trailing ? <Icon name={trailing} size={24} className="ds-field-trail" /> : null}
      </div>
      {error || helper ? <div className="ds-field-helper">{error ? <Icon name="error" fill size={14} /> : null}{error || helper}</div> : null}
    </div>
  );
}

/* ---------- Feedback ---------- */
export function Dialog({ icon, title, children, confirm = 'Confirm', cancel = 'Cancel', danger }) {
  return (
    <div className="ds-dialog" role="dialog" aria-modal="true">
      {icon ? <Icon name={icon} size={24} className={cx('ds-dialog-icon', danger && 'is-danger')} /> : null}
      <h2 className="ds-dialog-title">{title}</h2>
      <div className="ds-dialog-body">{children}</div>
      <div className="ds-dialog-actions">
        <Button variant="text">{cancel}</Button>
        <Button variant={danger ? 'danger' : 'filled'}>{confirm}</Button>
      </div>
    </div>
  );
}

export function Snackbar({ children, action = 'Undo' }) {
  return (
    <div className="ds-snackbar" role="status">
      <span>{children}</span>
      {action ? <button type="button" className="ds-snackbar-action">{action}</button> : null}
    </div>
  );
}

/* ---------- Onboarding ---------- */
export function PermissionStep({ icon, title, why, state = 'needed', optional, cta = 'Allow' }) {
  return (
    <div className={cx('ds-perm', 'is-' + state)}>
      <span className="ds-perm-icon"><Icon name={icon} size={24} /></span>
      <span className="ds-perm-text">
        <span className="ds-perm-title">{title}{optional ? <span className="ds-perm-opt">Optional</span> : null}</span>
        <span className="ds-perm-why">{why}</span>
      </span>
      {state === 'granted'
        ? <span className="ds-perm-done"><Icon name="check_circle" fill size={20} />Allowed</span>
        : state === 'next' ? <Button variant="filled" size="sm">{cta}</Button>
          : <Button variant="outlined" size="sm">{cta}</Button>}
    </div>
  );
}

/* ---------- Showcase-only device frame ---------- */
export function PhoneFrame({ children, time = '9:41', notif, label, width = 360, height = 780, overlay }) {
  return (
    <figure className="ds-phone-fig">
      <div className="ds-phone" style={{ width, height }}>
        <div className="ds-phone-status">
          <span>{time}</span>
          {notif ? <span className="ds-phone-notif"><Logo variant="notification" size={16} /></span> : null}
          <span className="ds-phone-sys">
            <Icon name="wifi" fill size={16} /><Icon name="signal_cellular_alt" size={16} /><span className="ds-phone-batt" />
          </span>
        </div>
        <div className="ds-phone-screen">{children}</div>
        {overlay}
        <div className="ds-phone-gesture"><span /></div>
      </div>
      {label ? <figcaption className="ds-phone-cap">{label}</figcaption> : null}
    </figure>
  );
}

/** Scrollable body + optional bottom bar inside a PhoneFrame */
export function Screen({ appbar, children, bottom, bottomPlain, fab, nav, tint, overlay }) {
  return (
    <div className={cx('ds-screen', tint && 'tint-' + tint)}>
      {appbar}
      <div className="ds-screen-body">{children}</div>
      {fab ? <div className="ds-screen-fab">{fab}</div> : null}
      {bottom ? <div className={cx('ds-screen-bottom', bottomPlain && 'is-plain')}>{bottom}</div> : null}
      {nav}
      {overlay}
    </div>
  );
}

/** Showcase-only page wrapper for the screen cards */
export function Showcase({ title, intro, notes = [], children, narrow }) {
  return (
    <div className={cx('ds-showcase', narrow && 'is-narrow')}>
      <header className="ds-showcase-head">
        <div className="ds-showcase-title">
          <h2>{title}</h2>
          {intro ? <p>{intro}</p> : null}
        </div>
        {notes.length ? (
          <ul className="ds-showcase-notes">
            {notes.map((n, i) => <li key={i}><strong>{n[0]}</strong>{n[1]}</li>)}
          </ul>
        ) : null}
      </header>
      <div className="ds-showcase-row">{children}</div>
    </div>
  );
}
