// Tiny IndexedDB key-value store for cached terminal screens (kept on this device only).
const DB = 'briardr';
const STORE = 'screens';
let dbp: Promise<IDBDatabase> | null = null;

function db(): Promise<IDBDatabase> {
  if (!dbp)
    dbp = new Promise((resolve, reject) => {
      const r = indexedDB.open(DB, 1);
      r.onupgradeneeded = () => r.result.createObjectStore(STORE);
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(r.error);
    });
  return dbp;
}

export async function idbGet<T>(key: string): Promise<T | undefined> {
  try {
    const d = await db();
    return await new Promise((resolve) => {
      const r = d.transaction(STORE, 'readonly').objectStore(STORE).get(key);
      r.onsuccess = () => resolve(r.result as T);
      r.onerror = () => resolve(undefined);
    });
  } catch {
    return undefined;
  }
}

export async function idbSet(key: string, value: unknown): Promise<void> {
  try {
    const d = await db();
    await new Promise<void>((resolve) => {
      const tx = d.transaction(STORE, 'readwrite');
      tx.objectStore(STORE).put(value, key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => resolve();
    });
  } catch {}
}

export interface CachedScreen {
  data: string;
  cols: number;
  rows: number;
  savedAt: number;
}

/**
 * Removes cached screens of streams not opened for 30 days, and beyond the 100 most recent. A stream
 * that's open saves its screen continually, so it's never the oldest.
 */
export async function pruneScreens(keep = 100, maxAgeMs = 30 * 24 * 3600_000): Promise<number> {
  try {
    const d = await db();
    return await new Promise((resolve) => {
      const tx = d.transaction(STORE, 'readwrite');
      const store = tx.objectStore(STORE);
      const all: { key: IDBValidKey; savedAt: number }[] = [];
      const cursor = store.openCursor();
      cursor.onsuccess = () => {
        const c = cursor.result;
        if (c) {
          all.push({ key: c.key, savedAt: (c.value as CachedScreen)?.savedAt ?? 0 });
          return c.continue();
        }
        all.sort((a, b) => b.savedAt - a.savedAt);
        const old = all.filter((x, i) => i >= keep || Date.now() - x.savedAt > maxAgeMs);
        for (const x of old) store.delete(x.key);
        tx.oncomplete = () => resolve(old.length);
      };
      tx.onerror = () => resolve(0);
    });
  } catch {
    return 0;
  }
}
