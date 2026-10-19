// Long-press (touch) or context-menu (pointer) handlers, to spread onto an element.

export function longPress(onLong: (pt: { x: number; y: number }) => void, ms = 480) {
  let timer: number | null = null;
  let start: { x: number; y: number } | null = null;
  let fired = false;
  const clear = () => {
    if (timer) clearTimeout(timer);
    timer = null;
  };
  return {
    handlers: {
      onpointerdown: (e: PointerEvent) => {
        if (e.pointerType === 'mouse') return;
        fired = false;
        start = { x: e.clientX, y: e.clientY };
        clear();
        timer = window.setTimeout(() => {
          fired = true;
          navigator.vibrate?.(10);
          onLong(start!);
        }, ms);
      },
      onpointermove: (e: PointerEvent) => {
        if (start && Math.hypot(e.clientX - start.x, e.clientY - start.y) > 10) clear();
      },
      onpointerup: clear,
      onpointercancel: clear,
      oncontextmenu: (e: MouseEvent) => {
        e.preventDefault();
        clear();
        if (!fired) onLong({ x: e.clientX, y: e.clientY });
        fired = false;
      },
    },
    /** True if the click that follows a long press should be ignored. */
    consumeClick: () => {
      const f = fired;
      fired = false;
      return f;
    },
  };
}
