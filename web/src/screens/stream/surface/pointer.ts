// Fingers, the mouse and the wheel on the terminal; the keyboard goes to xterm's own input.
//  - A tap opens a link, a hold opens the menu of the cell held, and a swipe scrolls (touch.ts).
//  - The mouse is as in a real terminal, and its right button opens the menu (mouse.ts).
//  - A wheel or trackpad scrolls, captured before xterm sees it: with no scrollback, xterm would turn the
//    wheel into arrow keys.
// Scrolling sends one scroll event per row crossed, anchored at a cell (scroll.ts); the controller drops
// it, never queues it, unless this device is controlling.
//
// On a touch screen the key bar's keyboard key opens and closes the on-screen keyboard: a tap opens a
// link and a swipe scrolls, both leaving the keyboard as it is. With the bar hidden for a hardware
// keyboard, where no on-screen one shows, a tap puts the focus in the terminal instead. iOS follows a
// tap with a mousedown, on which xterm would focus its input (opening the keyboard) or the page would
// take focus (closing it), so that mousedown stops here.
import { toast } from '../../../store.ts';
import type { TerminalController } from '../../../terminal/controller.ts';
import { logicalLineAt, screenSelection } from '../../../terminal/links.ts';
import { openMenu, type MenuItem } from '../../../ui/kit.ts';
import { mouseInput } from './mouse.ts';
import { RowScroll, wheelPixels } from './scroll.ts';
import type { SurfaceSheet } from './Sheets.svelte';
import { touchGestures, type Cell } from './touch.ts';

export interface PointerOptions {
  /** The key bar shows. Without it, a tap puts the focus in the terminal. */
  keyBar: () => boolean;
  /** Opens a sheet over the terminal (Select Text). */
  open: (sheet: SurfaceSheet) => void;
}

const copy = (text: string, msg: string) =>
  navigator.clipboard.writeText(text).then(
    () => toast(msg),
    () => toast('Copying isn’t allowed here.'),
  );

/** The terminal host's pointer handlers, and `dispose` to end a mouse press when the terminal goes. */
export function terminalPointer(ctl: TerminalController, opts: PointerOptions) {
  const openLink = (url: string) => ctl.opts.onLink(url);

  const linkMenu = (url: string): MenuItem[] => [
    { label: 'Open Link', icon: 'link', onSelect: () => openLink(url) },
    { label: 'Copy Link', icon: 'copy', onSelect: () => copy(url, 'Link copied') },
  ];

  const textMenu = ({ col, row }: Cell): MenuItem[] => {
    const abs = ctl.term.buffer.active.viewportY + row;
    return [
      { label: 'Copy Line', icon: 'copy', onSelect: () => copy(logicalLineAt(ctl.term, abs).text.trimEnd(), 'Line copied') },
      { label: 'Select Text…', icon: 'clipboard', onSelect: () => opts.open({ k: 'select', ...screenSelection(ctl.term, row, col) }) },
    ];
  };

  const scroll = (rows: number, cell: Cell) => {
    for (; rows; rows -= Math.sign(rows)) ctl.scroll(rows < 0 ? 'up' : 'down', cell.col, cell.row);
  };

  const touch = touchGestures(ctl, {
    tap(cell) {
      const link = ctl.linkAt(cell.col, cell.row);
      if (link) return openLink(link);
      if (!opts.keyBar()) ctl.focus();
    },
    hold(cell, at) {
      const link = ctl.linkAt(cell.col, cell.row);
      openMenu(link ? linkMenu(link) : textMenu(cell), at, link || undefined);
    },
    scroll,
  });
  const mouse = mouseInput(ctl, (sel) => copy(sel, 'Copied'));
  const wheel = new RowScroll();
  let lastPointer = 'mouse';

  return {
    handlers: {
      onpointerdown(e: PointerEvent) {
        lastPointer = e.pointerType;
        if (e.pointerType === 'mouse') mouse.press(e);
        else touch.down(e);
      },
      onpointermove(e: PointerEvent) {
        if (e.pointerType === 'mouse') mouse.move(e);
        else touch.move(e);
      },
      onpointerup: touch.up,
      onpointercancel: touch.cancel,
      onmousedowncapture(e: MouseEvent) {
        if (lastPointer === 'mouse') return;
        e.preventDefault();
        e.stopPropagation();
      },
      onwheelcapture(e: WheelEvent) {
        e.preventDefault();
        e.stopPropagation();
        const h = ctl.cell().h;
        scroll(wheel.add(wheelPixels(e, h), h), ctl.cellAt(e.clientX, e.clientY));
      },
      oncontextmenu(e: MouseEvent) {
        e.preventDefault();
        const cell = ctl.cellAt(e.clientX, e.clientY);
        const sel = ctl.term.getSelection();
        const link = ctl.linkAt(cell.col, cell.row);
        const items = link ? linkMenu(link) : textMenu(cell);
        if (sel) items.unshift({ label: 'Copy Selection', icon: 'copy', onSelect: () => copy(sel, 'Copied') });
        openMenu(items, { x: e.clientX, y: e.clientY });
      },
    },
    dispose: mouse.end,
  };
}
