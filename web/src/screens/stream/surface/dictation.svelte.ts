// Dictation: holding the key bar's Enter records speech until released, where the host transcribes it (a
// Mac with its on-device speech helper). The text goes into the input line as one paste, never submitted,
// and only on the connection the recording started on.
import { api, ApiError } from '../../../api.ts';
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
      this.active = true;
      const tick = () => {
        if (this.#rec !== r) return;
        this.level = r.level;
        requestAnimationFrame(tick);
      };
      tick();
    } catch {
      this.#rec = null;
      toast('Microphone access was denied. Allow it in Settings to dictate.');
    }
  };

  /** Ends the recording: transcribed and inserted, or with `cancel` dropped. */
  end = async (cancel: boolean) => {
    const r = this.#rec;
    this.#rec = null;
    this.active = false;
    if (!r) return;
    if (cancel) return r.cancel();
    const wav = await r.stop();
    const t0 = performance.now();
    try {
      const res = await api<Dictated>('POST', `/api/dictation?locale=${encodeURIComponent(navigator.language)}`, undefined, { raw: wav, headers: { 'content-type': 'audio/wav' }, timeout: 60_000 });
      record('dictation_done', { ms: Math.round(performance.now() - t0) });
      if (this.#ctl.paste(res.text, true, this.#generation)) toast('Dictation inserted. Edit, then press Enter.');
    } catch (e) {
      toast(e instanceof ApiError ? e.message : 'Dictation failed.', 5000);
    }
  };
}
