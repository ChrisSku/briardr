// Briardr was called Kelpie until October 2026. This moves what a device kept under the old name (settings,
// notes, plugin data) to the new keys and drops the old screen cache. Modules that read localStorage as
// they load (diag.ts, reactive.svelte.ts) import this first: the bundle's shared chunks run before main.ts.
// The service worker removes the old caches (web/src/sw.ts).
try {
  for (const key of Object.keys(localStorage)) {
    if (!key.startsWith('kelpie.')) continue;
    const next = 'briardr.' + key.slice('kelpie.'.length);
    const value = localStorage.getItem(key);
    if (localStorage.getItem(next) === null && value !== null) localStorage.setItem(next, value);
    localStorage.removeItem(key);
  }
} catch {}
try {
  indexedDB.deleteDatabase('kelpie');
} catch {}
