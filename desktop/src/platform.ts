// Which desktop the window runs on, for kahf-design's desktop layer (`data-kf-platform` on the root): macOS's
// WKWebView or Windows' WebView2. Linux's WebKitGTK takes the Windows path. The frame and the keys differ per
// platform; inside the window both look the same (#124, app.css).

export type Platform = 'mac' | 'windows';

export function platformOf(userAgent: string): Platform {
  return /Macintosh|Mac OS X/.test(userAgent) ? 'mac' : 'windows';
}
