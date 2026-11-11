import { describe, expect, it } from 'vitest';
import { platformOf } from './platform';

describe('platformOf', () => {
  it('reads macOS from WKWebView', () => {
    expect(platformOf('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)')).toBe('mac');
  });

  it('reads Windows from WebView2', () => {
    expect(platformOf('Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0')).toBe('windows');
  });

  it('gives Linux the Windows density', () => {
    expect(platformOf('Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)')).toBe('windows');
  });
});
