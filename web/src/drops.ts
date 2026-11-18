// Files dragged over Briardr. Where nothing on the page takes them (the terminal, a note, a plugin), the
// browser opens a dropped file in place of Briardr; the app refuses those drops instead.

/** The drag carries files, not text or a link. */
export const hasFiles = (e: DragEvent) => !!e.dataTransfer?.types.includes('Files');

/** Refuses files dragged where nothing takes them; the app does this once at startup. It listens on the
 *  window, where the events end as they bubble, so every handler on the page has had them first and it
 *  only refuses what none of them accepted (with preventDefault). Text and links are left to the browser. */
export function refuseStrayDrops() {
  // A file input takes files by itself.
  const stray = (e: DragEvent) =>
    hasFiles(e) && !e.defaultPrevented && !(e.target instanceof HTMLInputElement && e.target.type === 'file');
  window.addEventListener('dragover', (e) => {
    if (!stray(e)) return;
    e.preventDefault();
    e.dataTransfer!.dropEffect = 'none'; // shows they can't be dropped here
  });
  window.addEventListener('drop', (e) => {
    if (stray(e)) e.preventDefault();
  });
}
