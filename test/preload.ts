// Loaded before every test file (bunfig.toml), so its afterAll runs once, when the whole run ends: it
// removes the Herdr session and the folders the run used (test/helpers.ts).
import { afterAll } from 'bun:test';
import { endRun } from './helpers.ts';

// Servers under test never ask GitHub for releases: their update check goes to a closed port instead
// (test/update.test.ts serves its own).
process.env.BRIARDR_RELEASES_URL ||= 'http://127.0.0.1:9';

afterAll(endRun);
