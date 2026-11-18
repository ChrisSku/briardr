// The mouse on the terminal, as in a real terminal: a program that asked for the mouse gets clicks, drags
// and movement (Herdr drops them for one that didn't, so a shell never sees them), Shift keeps a drag from
// reaching the program, and the right button opens the menu, as it does in Herdr. Herdr doesn't say which
// programs asked, so the terminal's own selection is always there, and what it selects is copied on
// release, as in Herdr.
import type { TerminalController } from '../../../terminal/controller.ts';
import type { Cell } from './touch.ts';

/** The modifiers held, as Herdr takes them with a mouse event. */
export const modifiers = (e: Pick<MouseEvent, 'shiftKey' | 'ctrlKey' | 'altKey'>) => (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0);

/** The button a press sends, or null for one that opens the menu: the right button, and on a Mac a
 *  Control-click, which is the right button there. */
export function pressedButton(e: Pick<MouseEvent, 'button' | 'ctrlKey'>, mac: boolean): 'left' | 'middle' | null {
  return e.button === 0 && !(e.ctrlKey && mac) ? 'left' : e.button === 1 ? 'middle' : null;
}

/** The pointer handlers for the mouse; `onSelected` gets what the left button selected, on release. */
export function mouseInput(ctl: Pick<TerminalController, 'cellAt' | 'mouse' | 'term'>, onSelected: (text: string) => void) {
  let endPress: (() => void) | null = null;
  let hover: Cell = { col: -1, row: -1 };
  return {
    press(e: PointerEvent) {
      const button = pressedButton(e, navigator.platform.startsWith('Mac'));
      if (!button) return;
      endPress?.();
      const send = !e.shiftKey;
      let last = ctl.cellAt(e.clientX, e.clientY);
      if (send) ctl.mouse('down', button, last.col, last.row, modifiers(e));
      const move = (m: PointerEvent) => {
        const c = ctl.cellAt(m.clientX, m.clientY);
        if (c.col === last.col && c.row === last.row) return;
        last = c;
        if (send) ctl.mouse('drag', button, c.col, c.row, modifiers(m));
      };
      // On the window, so a drag that ends outside the terminal counts, and after xterm's own handler,
      // so its selection is final. Copying has to happen in this event: Safari only allows it in one.
      const up = (m: MouseEvent) => {
        if (m.button !== e.button) return;
        release(modifiers(m));
        const sel = button === 'left' && ctl.term.getSelection();
        if (sel) onSelected(sel);
      };
      const release = (mods: number) => {
        endPress = null;
        hover = last;
        window.removeEventListener('pointermove', move);
        window.removeEventListener('mouseup', up);
        window.removeEventListener('blur', lost);
        if (send) ctl.mouse('up', button, last.col, last.row, mods);
      };
      const lost = () => release(0); // the release will never come: don't leave the program mid-drag
      endPress = lost;
      window.addEventListener('pointermove', move);
      window.addEventListener('mouseup', up);
      window.addEventListener('blur', lost);
    },
    /** Movement with no button pressed. */
    move(e: PointerEvent) {
      if (endPress) return; // a drag, handled above
      const c = ctl.cellAt(e.clientX, e.clientY);
      if (c.col === hover.col && c.row === hover.row) return;
      hover = c;
      ctl.mouse('move', 'left', c.col, c.row, modifiers(e));
    },
    /** Ends a press whose release won't come now. */
    end() {
      endPress?.();
    },
  };
}
