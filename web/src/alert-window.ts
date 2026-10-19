// Where a tapped notification opens (used by sw.ts): an open window, chosen so the tap lands where the
// user expects, or a new one at the alert's stream.

/** The path a notification for `streamId` opens (`from=alert`: open that exact stream, router.ts). */
export function alertPath(streamId: string | null): string {
  return streamId ? `/stream/${encodeURIComponent(streamId)}?from=alert` : '/';
}

/**
 * The window to bring forward: one already showing that stream, else the focused one, else a visible
 * one, else the first (the most recently focused, where the browser orders them). None: open a window.
 */
export function pickWindow<W extends { url: string; focused: boolean; visibilityState: string }>(wins: readonly W[], streamId: string | null): W | undefined {
  const showing = streamId ? `/stream/${encodeURIComponent(streamId)}` : null;
  const on = (w: W) => showing !== null && (new URL(w.url).pathname === showing || new URL(w.url).pathname.startsWith(showing + '/'));
  return wins.find(on) ?? wins.find((w) => w.focused) ?? wins.find((w) => w.visibilityState === 'visible') ?? wins[0];
}
