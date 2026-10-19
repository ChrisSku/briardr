// Shared reactive values for plain TypeScript modules (store, router, terminal controller, live
// connection). Each is a small box with a `.value`, backed by Svelte's $state.raw: reading `.value` in a
// component template or $derived/$effect subscribes to it, and assigning a new value notifies readers.
// Values are replaced, never mutated in place, so no deep proxy is needed (a model with 500 streams
// stays a plain object).
import './rename.ts'; // before the first read of this device's storage
import { record } from './diag.ts';

export class State<T> {
  #v = $state.raw() as T;
  constructor(initial: T) {
    this.#v = initial;
  }
  get value(): T {
    return this.#v;
  }
  set value(v: T) {
    this.#v = v;
  }
}

export function state<T>(initial: T): State<T> {
  return new State(initial);
}

/**
 * A value kept in this browser's localStorage under `key`, saved on every assignment. With `throttle`
 * (ms), saved at most that often (the latest value, at the end of the interval) and whenever the page
 * is hidden: for large values that change often, like the cached model during a burst of updates.
 */
export class Persisted<T> extends State<T> {
  readonly key: string;
  #throttle: number;
  #timer: ReturnType<typeof setTimeout> | null = null;
  constructor(key: string, initial: T, throttle = 0) {
    super(load(key, initial));
    this.key = key;
    this.#throttle = throttle;
    // Store the initial value if nothing valid was stored. A loaded value isn't written back: that would
    // serialize the whole cached model again at every launch.
    if (this.value === initial) save(key, initial);
    if (throttle) {
      document.addEventListener('visibilitychange', () => document.visibilityState === 'hidden' && this.flush());
      addEventListener('pagehide', () => this.flush());
    }
  }
  override get value(): T {
    return super.value;
  }
  override set value(v: T) {
    super.value = v;
    if (!this.#throttle) save(this.key, v);
    else this.#timer ??= setTimeout(() => this.flush(), this.#throttle);
  }
  /** Save a change that is waiting for the throttle interval now. */
  flush() {
    if (this.#timer === null) return;
    clearTimeout(this.#timer);
    this.#timer = null;
    save(this.key, this.value);
  }
}

export function persisted<T>(key: string, initial: T, throttle = 0): Persisted<T> {
  return new Persisted(key, initial, throttle);
}

/** A cached computation over other reactive values, recomputed when they change. */
export class Derived<T> {
  #fn: () => T;
  #v = $derived.by(() => this.#fn());
  constructor(fn: () => T) {
    this.#fn = fn;
  }
  get value(): T {
    return this.#v;
  }
}

export function derived<T>(fn: () => T): Derived<T> {
  return new Derived(fn);
}

export function load<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : (JSON.parse(v) as T);
  } catch {
    return fallback;
  }
}

const failed = new Set<string>();
/** Writes a value; false if this browser couldn't store it (full, or storage not allowed). */
export function save(key: string, value: unknown): boolean {
  try {
    localStorage.setItem(key, JSON.stringify(value));
    failed.delete(key);
    return true;
  } catch (e) {
    // Once per key until it works again, as a category (diagnostics never hold the key or the value).
    if (!failed.has(key)) record('storage_write_failed', { category: e instanceof DOMException && e.name === 'QuotaExceededError' ? 'quota' : 'unavailable' });
    failed.add(key);
    return false;
  }
}
