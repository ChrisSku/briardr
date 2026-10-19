// Device-side media helpers.
//  - Recorder: captures microphone audio as 16 kHz mono PCM WAV for on-Mac, on-device transcription.
//  - prepareImage: fixes orientation, drops camera/location metadata (by re-encoding pixels only),
//    converts to PNG, caps the longest side at 4096 px and the size at 20 MB.

export const MAX_IMAGE_SIDE = 4096;
export const MAX_IMAGE_BYTES = 20 * 1024 * 1024;

export interface PreparedImage {
  blob: Blob;
  width: number;
  height: number;
  name: string;
  url: string;
  scaled: boolean;
}

export async function prepareImage(file: Blob, name = 'image'): Promise<PreparedImage> {
  // 'from-image' applies the EXIF orientation; drawing to a canvas keeps pixels and nothing else.
  const bmp = await createImageBitmap(file, { imageOrientation: 'from-image' } as ImageBitmapOptions);
  let { width, height } = bmp;
  const longest = Math.max(width, height);
  const scaled = longest > MAX_IMAGE_SIDE;
  if (scaled) {
    const k = MAX_IMAGE_SIDE / longest;
    width = Math.round(width * k);
    height = Math.round(height * k);
  }
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext('2d')!;
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(bmp, 0, 0, width, height);
  bmp.close();
  let blob: Blob = await new Promise((resolve, reject) => canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('Could not encode PNG'))), 'image/png'));
  // Very large photos can exceed 20 MB as PNG; step down until they fit.
  let w = width;
  let h = height;
  while (blob.size > MAX_IMAGE_BYTES && w > 512) {
    w = Math.round(w * 0.8);
    h = Math.round(h * 0.8);
    const c2 = document.createElement('canvas');
    c2.width = w;
    c2.height = h;
    c2.getContext('2d')!.drawImage(canvas, 0, 0, w, h);
    blob = await new Promise((resolve, reject) => c2.toBlob((b) => (b ? resolve(b) : reject(new Error('Could not encode PNG'))), 'image/png'));
  }
  const base = (name || 'image').replace(/\.[^.]+$/, '').replace(/[^\w.-]+/g, '-').slice(0, 60) || 'image';
  return { blob, width: w, height: h, name: `${base}.png`, url: URL.createObjectURL(blob), scaled: scaled || w !== width };
}

export class Recorder {
  private ctx: AudioContext | null = null;
  private stream: MediaStream | null = null;
  private node: ScriptProcessorNode | null = null;
  private chunks: Float32Array[] = [];
  private rate = 48000;
  level = 0;

  async start(): Promise<void> {
    this.stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
    const Ctx = window.AudioContext || (window as any).webkitAudioContext;
    this.ctx = new Ctx();
    this.rate = this.ctx.sampleRate;
    const src = this.ctx.createMediaStreamSource(this.stream);
    // ScriptProcessor is deprecated but universally available (incl. iOS Safari) and fine for short clips.
    this.node = this.ctx.createScriptProcessor(4096, 1, 1);
    this.node.onaudioprocess = (e) => {
      const d = e.inputBuffer.getChannelData(0);
      this.chunks.push(new Float32Array(d));
      let peak = 0;
      for (let i = 0; i < d.length; i += 64) peak = Math.max(peak, Math.abs(d[i]));
      this.level = peak;
    };
    src.connect(this.node);
    this.node.connect(this.ctx.destination);
  }

  async stop(): Promise<Blob> {
    this.node?.disconnect();
    this.stream?.getTracks().forEach((t) => t.stop());
    await this.ctx?.close();
    const total = this.chunks.reduce((n, c) => n + c.length, 0);
    const pcm = new Float32Array(total);
    let o = 0;
    for (const c of this.chunks) {
      pcm.set(c, o);
      o += c.length;
    }
    this.chunks = [];
    return encodeWav(downsample(pcm, this.rate, 16000), 16000);
  }

  cancel() {
    this.node?.disconnect();
    this.stream?.getTracks().forEach((t) => t.stop());
    this.ctx?.close();
    this.chunks = [];
  }
}

export function downsample(input: Float32Array, from: number, to: number): Float32Array {
  if (from === to) return input;
  const ratio = from / to;
  const out = new Float32Array(Math.floor(input.length / ratio));
  for (let i = 0; i < out.length; i++) {
    const start = Math.floor(i * ratio);
    const end = Math.min(input.length, Math.floor((i + 1) * ratio));
    let sum = 0;
    for (let j = start; j < end; j++) sum += input[j];
    out[i] = sum / Math.max(1, end - start);
  }
  return out;
}

export function encodeWav(samples: Float32Array, rate: number): Blob {
  const buf = new ArrayBuffer(44 + samples.length * 2);
  const v = new DataView(buf);
  const str = (o: number, s: string) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
  str(0, 'RIFF');
  v.setUint32(4, 36 + samples.length * 2, true);
  str(8, 'WAVE');
  str(12, 'fmt ');
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  str(36, 'data');
  v.setUint32(40, samples.length * 2, true);
  for (let i = 0; i < samples.length; i++) v.setInt16(44 + i * 2, Math.max(-1, Math.min(1, samples[i])) * 0x7fff, true);
  return new Blob([buf], { type: 'audio/wav' });
}
