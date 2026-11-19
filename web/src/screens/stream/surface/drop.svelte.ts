// Files dropped on the terminal: offered for sending, like files picked with the + menu, but only to a
// terminal in your control (dropRefusal). Every drop Briardr handles is handled here; files dropped
// anywhere else are refused (refuseStrayDrops).
import { hasFiles } from '../../../drops.ts';
import { toast } from '../../../store.ts';
import type { TerminalController } from '../../../terminal/controller.ts';
import type { Stream } from '../../../types.ts';
import { droppedFiles, dropRefusal, uploadItems, type UploadItem } from '../insert.ts';

export class FileDrop {
  /** Files that would be offered are over the terminal: it shows the drop mark. */
  over = $state(false);
  #ctl: TerminalController;
  #stream: () => Stream;
  #onFiles: (items: UploadItem[]) => void;

  constructor(ctl: TerminalController, stream: () => Stream, onFiles: (items: UploadItem[]) => void) {
    this.#ctl = ctl;
    this.#stream = stream;
    this.#onFiles = onFiles;
  }

  #refusal() {
    return dropRefusal(this.#stream(), this.#ctl.status.value.ownership === 'controlling');
  }

  /** For the terminal's host element. */
  handlers = {
    ondragover: (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault(); // or the browser opens the file in place of Briardr
      this.over = !this.#refusal();
    },
    ondragleave: (e: DragEvent) => {
      // Moving onto the terminal's own parts leaves the host too.
      if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) this.over = false;
    },
    ondrop: (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      this.over = false;
      const refusal = this.#refusal();
      if (refusal) return toast(refusal);
      const { files, folders } = droppedFiles(e.dataTransfer!.items);
      if (folders) toast('Folders can’t be sent, only files.');
      if (files.length) this.#onFiles(uploadItems(files));
    },
  };
}
