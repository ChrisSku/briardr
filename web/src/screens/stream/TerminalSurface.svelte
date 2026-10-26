<!--
  The live terminal surface.
  Layout: the terminal is sized to the visible area without the on-screen keyboard. When the keyboard
  opens, the key bar slides up onto it, and the terminal slides up only as far as its content needs:
  the last line with text (an agent's input box, a prompt at the bottom) and the cursor stay in view,
  while a shell prompt near the top stays put. The Mac's terminal is never resized for the keyboard.
  If the Mac's screen is shorter than the view, it's aligned to the bottom.
-->
<script module lang="ts">
  import { record } from '../../diag.ts';
  import type { TerminalController } from '../../terminal/controller.ts';
  import { confirmDialog } from '../../ui/kit.ts';

  /** Takes control from another client after confirming; it switches to watching. */
  export async function confirmTakeOver(ctl: TerminalController) {
    const ok = await confirmDialog('Take over this stream?', takeOverBody, 'Take Over', true);
    if (ok) {
      record('term_takeover_confirmed', {});
      ctl.connect('control', true);
    }
  }
</script>

<script lang="ts">
  import { theHost } from '../../host.ts';
  import { onMount } from 'svelte';
  import { api, ApiError } from '../../api.ts';
  import { Recorder } from '../../media.ts';
  import { hardwareKeyboard, prefs, toast, topOverlay } from '../../store.ts';
  import { logicalLineAt, wordAround } from '../../terminal/links.ts';
  import type { Outgoing } from '../../terminal/keys.ts';
  import type { Stream } from '../../types.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import { openMenu, type MenuItem } from '../../ui/kit.ts';
  import { acceptsImages } from './insert.ts';
  import KeyBar from './keybar/KeyBar.svelte';
  import { keyboardInset } from '../../keyboard.svelte.ts';
  import PasteSheet from './PasteSheet.svelte';
  import SelectTextSheet from './SelectTextSheet.svelte';
  import UploadSheet from './UploadSheet.svelte';

  interface Props {
    ctl: TerminalController;
    stream: Stream;
    visible: boolean;
    compact: boolean;
  }
  let { ctl, stream, visible, compact }: Props = $props();

  let host: HTMLDivElement;
  let screen: HTMLDivElement;
  let shift: HTMLDivElement;
  let wrap: HTMLDivElement | undefined = $state();
  const kb = keyboardInset();
  // The key bar's home-indicator padding isn't needed above the keyboard: let it slide behind it so
  // the bar sits directly on the keyboard. The terminal moves by the same amount.
  const lift = $derived.by(() => {
    if (!kb.current) return 0;
    const safeBottom = wrap ? parseFloat(getComputedStyle(wrap).paddingBottom) || 0 : 0;
    return Math.max(0, kb.current - safeBottom);
  });
  let sheet = $state.raw<null | { k: 'paste'; text: string } | { k: 'upload'; file: File | Blob; kind: 'image' | 'file' } | { k: 'select'; text: string; start: number; end: number }>(null);
  let dictating = $state(false);
  let level = $state(0);
  let rec: Recorder | null = null;
  let dictGen = 0;
  const st = $derived(ctl.status.value);
  const controlling = $derived(st.ownership === 'controlling');
  let resized = $state(0); // bumped when the terminal takes a new size (e.g. the Mac's, while watching)
  let drawn = $state(0); // bumped by output while the keyboard is up: how far the screen moves depends on it

  onMount(() => {
    ctl.mount(host, screen);
    const sizes = ctl.term.onResize(() => resized++);
    // Paste from a hardware keyboard / context menu goes through review for files, directly for text.
    const onText = (e: Event) => {
      const text = (e as CustomEvent).detail as string;
      if (text.length > 2000 || text.includes('\n')) sheet = { k: 'paste', text };
      else ctl.paste(text, prefs.value.bracketedPaste);
    };
    const onFile = (e: Event) => {
      const f = (e as CustomEvent).detail as File | null;
      if (f) sheet = { k: 'upload', file: f, kind: f.type.startsWith('image/') ? 'image' : 'file' };
    };
    // Physical keyboard detection on touch devices: a keydown while no on-screen keyboard is showing.
    const onKey = () => {
      if ('ontouchstart' in window && kb.current === 0 && document.activeElement === ctl.term.textarea) hardwareKeyboard.value = true;
    };
    window.addEventListener('briardr:paste-text', onText);
    window.addEventListener('briardr:paste-file', onFile);
    window.addEventListener('keydown', onKey, true);
    return () => {
      sizes.dispose();
      window.removeEventListener('briardr:paste-text', onText);
      window.removeEventListener('briardr:paste-file', onFile);
      window.removeEventListener('keydown', onKey, true);
    };
  });

  $effect(() => {
    if (!kb.current) return;
    const sub = ctl.term.onWriteParsed(() => drawn++);
    return () => sub.dispose();
  });

  // Keep the content above the keyboard by pushing the top out of view, only as far as needed (see
  // raise). While watching, the screen has the Mac's size (we never resize it); scale it down to fit.
  $effect(() => {
    void [st, visible, resized, drawn];
    const inner = shift.querySelector<HTMLElement>('.xterm-screen');
    const scale = !controlling && inner && inner.offsetWidth > host.clientWidth ? host.clientWidth / inner.offsetWidth : 1;
    const up = lift ? raise(lift, scale) : 0;
    shift.style.transformOrigin = 'bottom left';
    shift.style.transform = [up ? `translateY(${-up}px)` : '', scale < 1 ? `scale(${scale})` : ''].join(' ').trim();
  });

  // How far the screen moves up when the key bar covers `lift` px of it. The screen is bottom-aligned
  // in its host: the blank rows under the last line with text (or the cursor) may go under the key
  // bar, and the screen never moves so far that the cursor leaves the top. So a shell prompt at the
  // top of a cleared screen stays put, and an agent's input box at the bottom sits on the key bar.
  function raise(lift: number, scale: number): number {
    const buf = ctl.term.buffer.active;
    const rows = ctl.term.rows;
    const row = (shift.offsetHeight * scale) / rows;
    let last = rows - 1;
    while (last > buf.cursorY && !buf.getLine(buf.viewportY + last)?.translateToString().trim()) last--;
    const blank = (rows - 1 - last) * row;
    const above = host.clientHeight - parseFloat(getComputedStyle(host).paddingTop) - rows * row + buf.cursorY * row;
    return Math.max(0, Math.min(lift - blank, above));
  }

  // ---------- gestures ----------
  let g: { id: number; x: number; y: number; lastY: number; acc: number; cell: { col: number; row: number }; moved: boolean; t: number; long: number | null } | null = null;

  const linkMenu = (url: string): MenuItem[] => [
    { label: 'Open Link', icon: 'link', onSelect: () => openLink(url) },
    { label: 'Copy Link', icon: 'copy', onSelect: () => copy(url, 'Link copied') },
  ];

  const textMenu = (row: number, col: number): MenuItem[] => {
    const abs = ctl.term.buffer.active.viewportY + row;
    return [
      { label: 'Copy Line', icon: 'copy', onSelect: () => copy(logicalLineAt(ctl.term, abs).text.trimEnd(), 'Line copied') },
      { label: 'Select Text…', icon: 'clipboard', onSelect: () => openSelect(row, col) },
    ];
  };

  function openSelect(row: number, col: number) {
    // A frozen copy of the screen: later output doesn't move the selection.
    const buf = ctl.term.buffer.active;
    const lines: string[] = [];
    let offset = 0;
    for (let r = 0; r < ctl.term.rows; r++) {
      const line = buf.getLine(buf.viewportY + r);
      const s = line ? line.translateToString(true) : '';
      if (r === row) offset = lines.join('\n').length + (r ? 1 : 0) + Math.min(col, s.length);
      lines.push(s);
    }
    const text = lines.join('\n');
    const w = wordAround(text, offset);
    sheet = { k: 'select', text, start: w.start, end: w.end };
  }

  const openLink = (url: string) => ctl.opts.onLink(url);

  const copy = (text: string, msg: string) =>
    navigator.clipboard.writeText(text).then(
      () => toast(msg),
      () => toast('Copying isn’t allowed here.'),
    );

  function onpointerdown(e: PointerEvent) {
    if (e.pointerType === 'mouse') return;
    const cell = ctl.cellAt(e.clientX, e.clientY);
    const long = window.setTimeout(() => {
      const cur = g;
      if (!cur || cur.moved) return;
      cur.long = null;
      g = null;
      navigator.vibrate?.(10);
      const link = ctl.linkAt(cell.col, cell.row);
      openMenu(link ? linkMenu(link) : textMenu(cell.row, cell.col), { x: e.clientX, y: e.clientY }, link ? link : undefined);
    }, 480);
    g = { id: e.pointerId, x: e.clientX, y: e.clientY, lastY: e.clientY, acc: 0, cell, moved: false, t: Date.now(), long };
  }

  function onpointermove(e: PointerEvent) {
    const cur = g;
    if (!cur || e.pointerId !== cur.id) return;
    if (!cur.moved && Math.hypot(e.clientX - cur.x, e.clientY - cur.y) > 8) {
      cur.moved = true;
      if (cur.long) clearTimeout(cur.long);
      cur.long = null;
      // A swipe while typing dismisses the keyboard and keeps scrolling the same connection.
      if (kb.current) ctl.blur();
    }
    if (!cur.moved) return;
    const dy = e.clientY - cur.lastY;
    cur.lastY = e.clientY;
    cur.acc += dy;
    const h = ctl.cell().h;
    while (Math.abs(cur.acc) >= h) {
      const dir = cur.acc > 0 ? 'up' : 'down'; // finger down → content scrolls up (wheel up)
      cur.acc -= Math.sign(cur.acc) * h;
      ctl.scroll(dir, cur.cell.col, cur.cell.row); // dropped (not queued) unless controlling
    }
  }

  function onpointerup(e: PointerEvent) {
    const cur = g;
    g = null;
    if (!cur || e.pointerId !== cur.id) return;
    if (cur.long) clearTimeout(cur.long);
    if (cur.moved) return;
    const link = ctl.linkAt(cur.cell.col, cur.cell.row);
    if (link) return openLink(link);
    if (controlling) ctl.focus();
  }

  function onpointercancel() {
    if (g?.long) clearTimeout(g.long);
    g = null;
  }

  // Mouse / trackpad: wheel → one scroll event per row crossed, anchored at the pointer's cell.
  let wheelAcc = 0;
  function onwheel(e: WheelEvent) {
    e.preventDefault();
    const h = ctl.cell().h;
    wheelAcc += e.deltaMode === 1 ? e.deltaY * h : e.deltaY;
    const cell = ctl.cellAt(e.clientX, e.clientY);
    while (Math.abs(wheelAcc) >= h) {
      const dir = wheelAcc < 0 ? 'up' : 'down';
      wheelAcc -= Math.sign(wheelAcc) * h;
      ctl.scroll(dir, cell.col, cell.row);
    }
  }

  function oncontextmenu(e: MouseEvent) {
    e.preventDefault();
    const cell = ctl.cellAt(e.clientX, e.clientY);
    const sel = ctl.term.getSelection();
    const link = ctl.linkAt(cell.col, cell.row);
    const items = link ? linkMenu(link) : textMenu(cell.row, cell.col);
    if (sel) items.unshift({ label: 'Copy Selection', icon: 'copy', onSelect: () => copy(sel, 'Copied') });
    openMenu(items, { x: e.clientX, y: e.clientY });
  }

  // ---------- + menu ----------
  function pickFile(accept: string, kind: 'image' | 'file') {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = accept;
    input.onchange = () => {
      const f = input.files?.[0];
      if (f) sheet = { k: 'upload', file: f, kind };
    };
    input.click();
  }

  async function pasteFromClipboard() {
    try {
      if ('read' in navigator.clipboard) {
        const items = await navigator.clipboard.read();
        for (const it of items) {
          const img = it.types.find((t) => t.startsWith('image/'));
          if (img) return (sheet = { k: 'upload', file: await it.getType(img), kind: 'image' });
        }
      }
      const text = await navigator.clipboard.readText();
      sheet = { k: 'paste', text };
    } catch {
      sheet = { k: 'paste', text: '' };
    }
  }

  function onPlus(anchor: HTMLElement) {
    openMenu(
      [
        { label: 'Paste from clipboard', icon: 'clipboard', onSelect: pasteFromClipboard, hint: 'Text or image' },
        { label: 'Paste text…', icon: 'clipboard', onSelect: () => (sheet = { k: 'paste', text: '' }) },
        { label: 'Select image…', icon: 'image', onSelect: () => pickFile('image/*', 'image'), hint: acceptsImages(stream) ? undefined : 'Inserts a path' },
        { label: 'Select file…', icon: 'file', onSelect: () => pickFile('*/*', 'file') },
      ],
      anchor,
      'Insert',
    );
  }

  // ---------- dictation (hold Enter) ----------
  async function dictateStart() {
    if (!controlling) return toast('Take control to dictate.');
    dictGen = ctl.generation.value;
    const r = new Recorder();
    rec = r;
    try {
      await r.start();
      dictating = true;
      const tick = () => {
        if (rec !== r) return;
        level = r.level;
        requestAnimationFrame(tick);
      };
      tick();
    } catch {
      rec = null;
      toast('Microphone access was denied. Allow it in Settings to dictate.');
    }
  }

  async function dictateEnd(cancel: boolean) {
    const r = rec;
    rec = null;
    dictating = false;
    if (!r) return;
    if (cancel) return r.cancel();
    const wav = await r.stop();
    const t0 = performance.now();
    try {
      const res = await api<{ text: string }>('POST', `/api/dictation?locale=${encodeURIComponent(navigator.language)}`, undefined, { raw: wav, headers: { 'content-type': 'audio/wav' }, timeout: 60_000 });
      record('dictation_done', { ms: Math.round(performance.now() - t0) });
      // One paste into the input line, never submitted, and only on the same connection.
      if (ctl.paste(res.text, true, dictGen)) toast('Dictation inserted. Edit, then press Enter.');
    } catch (e) {
      toast(e instanceof ApiError ? e.message : 'Dictation failed.', 5000);
    }
  }

  // ---------- ownership banner ----------
  // Watching can last; the banner covers the header, so it can be dismissed until the state changes.
  // Take control stays in the stream's actions menu. App shows it in the overlay above the header.
  const bannerKey = $derived(`${st.ownership}:${st.watchReason}`);
  let dismissed = $state<string | null>(null);
  $effect(() => {
    if (!visible) return;
    topOverlay.value = banner;
    return () => {
      if (topOverlay.value === banner) topOverlay.value = null;
    };
  });

  const isLive = $derived(controlling || st.ownership === 'watching');
  const showKeyBar = $derived(!hardwareKeyboard.value || compact);
</script>

{#snippet takeOverBody()}
  <p>Another client is controlling this pane. Taking over moves control to this device and resizes the pane for it. The other client will switch to watching.</p>
{/snippet}

{#snippet dismiss()}
  <button class="icon-btn" aria-label="Dismiss" onclick={() => (dismissed = bannerKey)}>
    <Icon name="close" size={16} />
  </button>
{/snippet}

{#snippet banner()}
  {#if st.ownership === 'watching' && dismissed === bannerKey}
    <!-- dismissed until the state changes -->
  {:else if st.ownership === 'watching' && (st.watchReason === 'busy' || st.watchReason === 'taken_over')}
    <div class="own-banner" role="status">
      <span>{st.watchReason === 'busy' ? 'Another client is controlling this stream. You’re watching.' : 'Another client took control. You’re watching.'}</span>
      <button class="btn btn-small" onclick={() => confirmTakeOver(ctl)}>Take Over</button>
      {@render dismiss()}
    </div>
  {:else if st.ownership === 'watching'}
    <div class="own-banner" role="status">
      <span>{st.watchReason === 'released' ? `Handed back to ${theHost()}. You’re watching.` : 'You’re watching.'}</span>
      <button class="btn btn-small" onclick={() => ctl.connect('control', false)}>Take control</button>
      {@render dismiss()}
    </div>
  {:else if st.ownership === 'reconnecting'}
    <div class="own-banner tone-warn" role="status">
      <span>{st.message || 'Reconnecting…'} Showing the last screen.</span>
      <button class="btn btn-small" onclick={() => ctl.retryNow()}>Retry now</button>
    </div>
  {:else if st.ownership === 'gone' || st.ownership === 'error'}
    <div class="own-banner tone-error" role="alert">
      <span>{st.message}</span>
      {#if st.ownership === 'error'}
        <button class="btn btn-small" onclick={() => location.reload()}>Reload</button>
      {/if}
    </div>
  {/if}
{/snippet}

<div class={['term-surface', !visible && 'is-hidden']} aria-hidden={!visible}>
  <!-- Pointer gestures on the terminal; the keyboard goes to xterm's own input. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class={['term-host', !isLive && 'is-stale']} bind:this={host} {onpointerdown} {onpointermove} {onpointerup} {onpointercancel} {onwheel} {oncontextmenu}>
    <div class="term-shift" bind:this={shift}>
      <div class="term-screen" bind:this={screen}></div>
    </div>
    {#if !isLive && (st.cachedAt || st.liveAt) && st.ownership !== 'connecting'}
      <div class="stale-tag" role="status">Last seen screen · not live</div>
    {/if}
  </div>
  {#if showKeyBar}
    <div class="keybar-wrap" bind:this={wrap} style:transform={lift ? `translateY(${-lift}px)` : undefined}>
      <KeyBar
        enabled={controlling}
        keyboardOpen={kb.current > 0}
        onKeyboard={() => (kb.current ? ctl.blur() : ctl.focus())}
        send={(o: Outgoing) => ctl.input(o)}
        {onPlus}
        onDictateStart={dictateStart}
        onDictateEnd={dictateEnd}
        {dictating}
        {level}
      />
    </div>
  {/if}
  {#if sheet?.k === 'paste'}<PasteSheet {ctl} {stream} initial={sheet.text} onClose={() => (sheet = null)} />{/if}
  {#if sheet?.k === 'upload'}<UploadSheet {ctl} {stream} file={sheet.file} kind={sheet.kind} onClose={() => (sheet = null)} />{/if}
  {#if sheet?.k === 'select'}<SelectTextSheet text={sheet.text} start={sheet.start} end={sheet.end} onClose={() => (sheet = null)} />{/if}
</div>
