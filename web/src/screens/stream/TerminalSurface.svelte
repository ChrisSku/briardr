<!--
  The live terminal surface.
  Layout: the terminal is sized to the visible area. When the on-screen keyboard opens, the surface
  ends on it: the key bar sits on the keyboard and, while controlling, the pane is resized to the rows
  above it, so a full-screen program (an agent with its input box at the bottom) lays itself out to
  fit. Until the new size arrives, the screen stays aligned to the bottom, keeping the input in view.
  If the Mac's screen is shorter than the view (watching), it's aligned to the bottom too.
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
  import { appOf } from '../../terminal/barkeys.ts';
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
  // The key bar's home-indicator padding isn't needed above the keyboard: the surface ends this far
  // above the bottom, so the bar sits directly on the keyboard and the terminal gets the rest.
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

  // While watching, the screen has the Mac's size (we never resize it); scale it down to fit.
  $effect(() => {
    void [st, visible, resized];
    const inner = shift.querySelector<HTMLElement>('.xterm-screen');
    const scale = !controlling && inner && inner.offsetWidth > host.clientWidth ? host.clientWidth / inner.offsetWidth : 1;
    shift.style.transformOrigin = 'bottom left';
    shift.style.transform = scale < 1 ? `scale(${scale})` : '';
  });

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

  // ---------- ownership banners ----------
  // Being in control is the normal case and isn't shown. Watching (another device has control) floats a
  // banner under the header, with Take over; it can be dismissed until the state changes. Reconnecting and
  // errors go in the overlay above the header (App shows it). Take control stays in the actions menu too.
  const bannerKey = $derived(`${st.ownership}:${st.watchReason}`);
  let dismissed = $state<string | null>(null);
  $effect(() => {
    if (!visible) return;
    topOverlay.value = banner;
    return () => {
      if (topOverlay.value === banner) topOverlay.value = null;
    };
  });
  const watching = $derived(st.ownership === 'watching' && dismissed !== bannerKey);
  const contested = $derived(st.watchReason === 'busy' || st.watchReason === 'taken_over');
  const watchText = $derived(
    st.watchReason === 'busy'
      ? 'Another device is in control'
      : st.watchReason === 'taken_over'
        ? 'Another device took control'
        : st.watchReason === 'released'
          ? `Handed back to ${theHost()}`
          : 'Read-only until you take control',
  );

  const isLive = $derived(controlling || st.ownership === 'watching');
  const showKeyBar = $derived(!hardwareKeyboard.value || compact);
</script>

{#snippet takeOverBody()}
  <p>Another client is controlling this pane. Taking over moves control to this device and resizes the pane for it. The other client will switch to watching.</p>
{/snippet}

{#snippet banner()}
  {#if st.ownership === 'reconnecting'}
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

<div class={['term-surface', !visible && 'is-hidden']} aria-hidden={!visible} style:bottom={lift ? `${lift}px` : undefined}>
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
  {#if watching}
    <div class="own-banner watch-banner" role="status">
      <span class="watch-tile" aria-hidden="true"><Icon name="eye" size={18} /></span>
      <span class="watch-text"><strong>Watching</strong><span>{watchText}</span></span>
      <button class="btn btn-small btn-primary" onclick={() => (contested ? confirmTakeOver(ctl) : ctl.connect('control', false))}>{contested ? 'Take over' : 'Take control'}</button>
      <button class="icon-btn" aria-label="Dismiss" onclick={() => (dismissed = bannerKey)}>
        <Icon name="close" size={16} />
      </button>
    </div>
  {/if}
  {#if showKeyBar}
    <div class={['keybar-wrap', kb.current && 'is-on-keyboard']} bind:this={wrap}>
      <KeyBar
        enabled={controlling}
        app={appOf(stream)}
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
