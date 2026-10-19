import fs from 'node:fs';
import path from 'node:path';
import { dataDir, emit, serve } from './briardr.ts';

let hits = 0;
serve(async (req) => {
  hits++;
  switch (req.path) {
    case '/hello':
      return { body: { hello: req.device.name, stream: req.stream?.id ?? null, cwd: req.stream?.cwd ?? null, query: req.query, hits, pid: process.pid } };
    case '/echo':
      return { body: { method: req.method, body: req.body, text: req.text ?? null, base64: req.base64 ?? null } };
    case '/text':
      return { text: 'plain words', headers: { 'content-type': 'text/plain; charset=utf-8', 'x-not-passed': 'no' } };
    case '/emit':
      emit('note', { hits });
      return { status: 204 };
    case '/data':
      fs.writeFileSync(path.join(dataDir, 'saved.txt'), 'kept');
      return { body: { dataDir } };
    case '/crash':
      process.exit(3);
    default:
      return { status: 404, body: { error: 'not_found' } };
  }
});
