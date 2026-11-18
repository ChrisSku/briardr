// Scrolling the terminal a row at a time: a swipe or a wheel moves some pixels at a time, and the program
// gets one scroll event per row crossed. What's left of a row carries over to the next move.

/** Pixels moved, as whole rows crossed. */
export class RowScroll {
  #px = 0;

  /** Adds `px` (as a wheel's deltaY: negative towards the top) and returns the rows crossed since the last
   *  call, negative towards the top. */
  add(px: number, rowHeight: number): number {
    this.#px += px;
    let rows = 0;
    while (Math.abs(this.#px) >= rowHeight) {
      const step = Math.sign(this.#px);
      this.#px -= step * rowHeight;
      rows += step;
    }
    return rows;
  }
}

/** A wheel event's distance in pixels: a mouse wheel may count it in lines. */
export const wheelPixels = (e: Pick<WheelEvent, 'deltaY' | 'deltaMode'>, rowHeight: number) => (e.deltaMode === 1 ? e.deltaY * rowHeight : e.deltaY);
