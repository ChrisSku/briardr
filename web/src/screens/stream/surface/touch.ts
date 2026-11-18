// A finger on the terminal: a tap, a hold or a swipe, all at the cell where it went down. A finger that
// moves past the slop is a swipe, which scrolls a row per row crossed and is never a tap or a hold; one
// that stays still long enough is a hold.
import type { TerminalController } from '../../../terminal/controller.ts';
import { RowScroll } from './scroll.ts';

/** How far a finger moves before it's a swipe, in CSS px. */
export const SWIPE_SLOP = 8;
/** How long a finger stays still before it's a hold, in ms. */
export const HOLD_MS = 480;

export interface Cell {
  col: number;
  row: number;
}

/** One finger's gesture, from down to up. */
export class TouchGesture {
  readonly id: number;
  readonly x: number;
  readonly y: number;
  /** Where the finger went down: a tap, a hold and a swipe all happen there. */
  readonly cell: Cell;
  /** It moved past the slop. */
  swipe = false;
  /** The timer that makes it a hold. */
  hold: ReturnType<typeof setTimeout> | null = null;
  #lastY: number;
  #rows = new RowScroll();

  constructor(id: number, x: number, y: number, cell: Cell) {
    this.id = id;
    this.x = x;
    this.y = y;
    this.cell = cell;
    this.#lastY = y;
  }

  /** Follows the finger to (x, y). Returns the rows a swipe crossed since the last move, negative towards
   *  the top: a finger moving down scrolls the content up, as a wheel turned up does. */
  move(x: number, y: number, rowHeight: number): number {
    if (!this.swipe && Math.hypot(x - this.x, y - this.y) > SWIPE_SLOP) {
      this.swipe = true;
      this.stopHold();
    }
    if (!this.swipe) return 0;
    const dy = y - this.#lastY;
    this.#lastY = y;
    return this.#rows.add(-dy, rowHeight);
  }

  stopHold() {
    if (this.hold) clearTimeout(this.hold);
    this.hold = null;
  }
}

export interface TouchActions {
  tap(cell: Cell): void;
  /** A finger held still on a cell; `at` is where on the screen, for a menu. */
  hold(cell: Cell, at: { x: number; y: number }): void;
  /** Rows a swipe crossed (negative towards the top), anchored at the cell where it started. */
  scroll(rows: number, cell: Cell): void;
}

/** The pointer handlers for fingers on the terminal. */
export function touchGestures(ctl: Pick<TerminalController, 'cellAt' | 'cell'>, actions: TouchActions) {
  let g: TouchGesture | null = null;
  return {
    down(e: PointerEvent) {
      const cell = ctl.cellAt(e.clientX, e.clientY);
      const at = { x: e.clientX, y: e.clientY };
      const touch = new TouchGesture(e.pointerId, e.clientX, e.clientY, cell);
      touch.hold = setTimeout(() => {
        const cur = g;
        if (!cur || cur.swipe) return;
        cur.hold = null;
        g = null;
        navigator.vibrate?.(10);
        actions.hold(cell, at);
      }, HOLD_MS);
      g = touch;
    },
    move(e: PointerEvent) {
      const cur = g;
      if (!cur || e.pointerId !== cur.id) return;
      const rows = cur.move(e.clientX, e.clientY, ctl.cell().h);
      if (rows) actions.scroll(rows, cur.cell);
    },
    up(e: PointerEvent) {
      const cur = g;
      g = null;
      if (!cur || e.pointerId !== cur.id) return;
      cur.stopHold();
      if (!cur.swipe) actions.tap(cur.cell);
    },
    cancel() {
      g?.stopHold();
      g = null;
    },
  };
}
