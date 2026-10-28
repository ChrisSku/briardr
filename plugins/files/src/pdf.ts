// PDF rendering for Files (pdf.js, its legacy build for older Safari). Loaded on demand with import(),
// like the highlighter, so it stays out of the main bundle. pdf.js's worker runs on the main thread:
// importing it sets globalThis.pdfjsWorker, which pdf.js picks up instead of starting a Worker (a plugin
// ships one bundle, and the page's CSP allows no blob: workers).
import { getDocument, PasswordException } from 'pdfjs-dist/legacy/build/pdf.mjs';
import 'pdfjs-dist/legacy/build/pdf.worker.mjs';

export type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist/legacy/build/pdf.mjs';

export async function openPdf(data: Uint8Array) {
  try {
    // No wasm decoders (the CSP allows no wasm) and no font downloads: pdf.js falls back to its own.
    return await getDocument({ data, useWasm: false, useSystemFonts: true }).promise;
  } catch (e) {
    throw new Error(e instanceof PasswordException ? 'This PDF is password-protected.' : 'Couldn’t read this PDF.');
  }
}
