// What kind of device this is, from what its browser says about itself. An iPad's Safari asks for
// desktop sites, so it says it's a Mac: only its touch screen gives it away.

export type Platform = 'iphone' | 'ipad' | 'mac-safari' | 'android' | 'firefox' | 'desktop';

/** What of `navigator` tells devices apart. */
type Nav = { userAgent: string; platform: string; maxTouchPoints: number };

export function platform(nav: Nav = navigator): Platform {
  const ua = nav.userAgent;
  if (/iPad/.test(ua) || (nav.platform === 'MacIntel' && nav.maxTouchPoints > 1)) return 'ipad';
  if (/iPhone|iPod/.test(ua)) return 'iphone';
  if (/Android/.test(ua)) return 'android';
  if (/Firefox\//.test(ua)) return 'firefox';
  if (/Macintosh/.test(ua) && /Safari\//.test(ua) && !/Chrome|Chromium|Edg\//.test(ua)) return 'mac-safari';
  return 'desktop';
}

/** An iPhone or iPad, whatever its browser (each is Safari underneath): web alerts reach only its Home
 *  Screen apps, which sign in separately from its browser. */
export function isIOS(p: Platform = platform()): boolean {
  return p === 'iphone' || p === 'ipad';
}

/** "iPhone" or "iPad" on one, null on anything else. */
export function iosDevice(p: Platform = platform()): 'iPhone' | 'iPad' | null {
  return p === 'iphone' ? 'iPhone' : p === 'ipad' ? 'iPad' : null;
}
