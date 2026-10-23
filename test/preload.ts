// Loaded before every test file (bunfig.toml), so its afterAll runs once, when the whole run ends: it
// removes the Herdr session and the temp directory the run used (test/helpers.ts).
import { afterAll } from 'bun:test';
import { endRun } from './helpers.ts';

afterAll(endRun);
