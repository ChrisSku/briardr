// Bounded exponential backoff with jitter. reset() is called when the network returns or changes.
export class Backoff {
  private attempt = 0;
  private base: number;
  private max: number;
  constructor(base = 500, max = 30_000) {
    this.base = base;
    this.max = max;
  }
  next(): number {
    const raw = Math.min(this.max, this.base * 2 ** this.attempt);
    this.attempt++;
    const jitter = raw * 0.2 * (Math.random() * 2 - 1);
    return Math.max(this.base, Math.round(raw + jitter));
  }
  reset(): void {
    this.attempt = 0;
  }
  get attempts(): number {
    return this.attempt;
  }
}

/** Calls fn whenever connectivity returns or the network path changes (e.g. Wi-Fi → cellular). */
export function onNetworkChange(fn: (kind: string) => void): () => void {
  const online = () => fn('online');
  const change = () => fn('change');
  window.addEventListener('online', online);
  const c = (navigator as any).connection;
  c?.addEventListener?.('change', change);
  return () => {
    window.removeEventListener('online', online);
    c?.removeEventListener?.('change', change);
  };
}
