// What opens the insert sheets, besides a drop: the key bar's + menu, and a paste into the terminal from a
// hardware keyboard or the context menu. Like every insert, nothing they open is ever submitted.
import { prefs } from '../../../store.ts';
import type { Pasted, TerminalController } from '../../../terminal/controller.ts';
import type { Stream } from '../../../types.ts';
import { openMenu } from '../../../ui/kit.ts';
import { acceptsImages, reviewsPaste, uploadItems } from '../insert.ts';
import type { SurfaceSheet } from './Sheets.svelte';

type Open = (sheet: SurfaceSheet) => void;

/** A paste into the terminal: files go through review, and so does text unless it's a short line. */
export function pasted(ctl: TerminalController, p: Pasted, open: Open) {
  if ('file' in p) open({ k: 'upload', items: uploadItems([p.file]) });
  else if (reviewsPaste(p.text)) open({ k: 'paste', text: p.text });
  else ctl.paste(p.text, prefs.value.bracketedPaste);
}

function pickFiles(accept: string, kind: 'image' | 'file', open: Open) {
  const input = document.createElement('input');
  input.type = 'file';
  input.accept = accept;
  input.multiple = true;
  input.onchange = () => {
    if (input.files?.length) open({ k: 'upload', items: Array.from(input.files, (file) => ({ file, kind })) });
  };
  input.click();
}

async function pasteFromClipboard(open: Open) {
  try {
    if ('read' in navigator.clipboard) {
      const items = await navigator.clipboard.read();
      for (const it of items) {
        const img = it.types.find((t) => t.startsWith('image/'));
        if (img) return open({ k: 'upload', items: [{ file: await it.getType(img), kind: 'image' }] });
      }
    }
    const text = await navigator.clipboard.readText();
    open({ k: 'paste', text });
  } catch {
    open({ k: 'paste', text: '' });
  }
}

/** The key bar's + menu, under its key. */
export function insertMenu(anchor: HTMLElement, stream: Stream, open: Open) {
  openMenu(
    [
      { label: 'Compose…', icon: 'pencil', onSelect: () => open({ k: 'compose' }), hint: 'Write here, then insert' },
      { label: 'Paste from clipboard', icon: 'clipboard', onSelect: () => pasteFromClipboard(open), hint: 'Text or image' },
      { label: 'Paste text…', icon: 'clipboard', onSelect: () => open({ k: 'paste', text: '' }) },
      // An upload lands on this host, where a terminal on another machine can't open it.
      ...(stream.machine
        ? []
        : [
            { label: 'Select image…', icon: 'image' as const, onSelect: () => pickFiles('image/*', 'image', open), hint: acceptsImages(stream) ? undefined : 'Inserts a path' },
            { label: 'Select file…', icon: 'file' as const, onSelect: () => pickFiles('*/*', 'file', open) },
          ]),
    ],
    anchor,
    'Insert',
  );
}
