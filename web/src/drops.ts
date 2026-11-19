// Files, links and text dragged over Briardr. Where nothing on the page takes them (the terminal, a note, a
// plugin, a field you can type in), the browser opens a dropped file or link in place of Briardr; the app
// refuses those drops instead.

/** The drag carries files, not text or a link. */
export const hasFiles = (e: DragEvent) => !!e.dataTransfer?.types.includes('Files');

/** What the browser itself puts where it's dropped: files into a file input, text and links into a field
 *  you can type in. Text is refused anywhere else too, because the browser opens one that's a web address,
 *  and a page can't read a drag's text before the drop. */
function takenByBrowser(e: DragEvent) {
  const t = e.target;
  return hasFiles(e) ? t instanceof HTMLInputElement && t.type === 'file' : t instanceof Element && t.matches(':read-write');
}

/** Refuses what's dragged where nothing takes it; the app does this once at startup. It listens on the
 *  window, where the events end as they bubble, so every handler on the page has had them first and it
 *  only refuses what none of them accepted (with preventDefault). */
export function refuseStrayDrops() {
  const stray = (e: DragEvent) => !!e.dataTransfer && !e.defaultPrevented && !takenByBrowser(e);
  window.addEventListener('dragover', (e) => {
    if (!stray(e)) return;
    e.preventDefault();
    e.dataTransfer!.dropEffect = 'none'; // shows it can't be dropped here
  });
  window.addEventListener('drop', (e) => {
    if (stray(e)) e.preventDefault();
  });
}
