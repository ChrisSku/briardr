// Dictation: holding the key bar's Enter records speech until released, where the host transcribes it (a
// Mac with its on-device speech helper). The text goes into the input line as one paste, never submitted,
// and only on the connection the recording started on.
import { api, errorMessage } from '../../../api.ts';
import { record } from '../../../diag.ts';
import { Recorder } from '../../../media.ts';
import type { Dictated } from '../../../protocol.ts';
import { toast } from '../../../store.ts';
import type { TerminalController } from '../../../terminal/controller.ts';

export class Dictation {
  /** Recording: Enter shows a microphone. */
  active = $state(false);
  /** The microphone's level while recording. */
  level = $state(0);
  #ctl: TerminalController;
  #rec: Recorder | null = null;
  #generation = 0;
  /** Aborted when the terminal goes away. */
  #gone = new AbortController();

  constructor(ctl: TerminalController) {
    this.#ctl = ctl;
  }

  start = async () => {
    if (this.#ctl.status.value.ownership !== 'controlling') return toast('Take control to dictate.');
    this.#generation = this.#ctl.generation.value;
    const r = new Recorder();
    this.#rec = r;
    try {
      await r.start();
    } catch {
      if (this.#rec !== r) return;
      this.#rec = null;
      return toast('Microphone access was denied. Allow it in Settings to dictate.');
    }
    // Let go, or the terminal went away, while the microphone was starting.
    if (this.#rec !== r) return r.cancel();
    this.active = true;
    const tick = () => {
      if (this.#rec !== r) return;
      this.level = r.level;
      requestAnimationFrame(tick);
    };
    tick();
  };

  /** Ends the recording: transcribed and inserted, or with `cancel` dropped. */
  end = async (cancel: boolean) => {
    const r = this.#rec;
    const recording = this.active;
    this.#rec = null;
    this.active = false;
    if (!r) return;
    // Let go before the microphone was on: nothing was recorded, and start lets go of it when it comes.
    if (cancel || !recording) return r.cancel();
    const wav = await r.stop();
    const t0 = performance.now();
    const gone = this.#gone.signal;
    try {
      const res = await api<Dictated>('POST', `/api/dictation?locale=${encodeURIComponent(navigator.language)}`, undefined, { raw: wav, headers: { 'content-type': 'audio/wav' }, timeout: 60_000, signal: gone });
      record('dictation_done', { ms: Math.round(performance.now() - t0) });
      if (gone.aborted) return;
      if (this.#ctl.paste(res.text, true, this.#generation)) toast('Dictation inserted. Edit, then press Enter.');
    } catch (e) {
      if (!gone.aborted) toast(errorMessage(e, 'Dictation failed.'), 5000);
    }
  };

  /** The terminal went away (the stream was left or closed): a recording is dropped and the microphone let
   *  go, and a transcription still on its way is never inserted. */
  dispose() {
    this.#gone.abort();
    void this.end(true);
  }
}
