// {{name}}'s backend: Briardr starts it on the first request and sends every request from a device here.
// It runs on the host as your user, in this folder. Logs (console.log) go to Briardr's data folder,
// logs/plugins/{{id}}.log. Keep data in `dataDir`.
import { emit, serve } from './briardr.ts';

serve(async (req) => {
  switch (`${req.method} ${req.path}`) {
    case 'GET /hello':
      return { body: { message: req.stream ? `Hello from ${req.stream.name} (${req.stream.cwd}).` : `Hello, ${req.device.name}.`, asOf: Date.now() } };
    case 'POST /ping':
      emit('ping', { at: Date.now() }); // every device's briardr.onEvent('ping', …) hears it
      return { status: 204 };
    default:
      return { status: 404, body: { error: 'not_found', message: `No ${req.method} ${req.path} here.` } };
  }
});
