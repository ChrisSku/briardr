// Loaded before every test file (bunfig.toml), so its afterAll runs once, when the whole run ends: it
// removes the Herdr session the run used (test/helpers.ts).
import { afterAll } from 'bun:test';
import { endSession } from './helpers.ts';

afterAll(endSession);
