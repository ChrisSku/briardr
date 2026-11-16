// Keys reach Herdr by name (it encodes them for the program's mode): hardware keydowns, and text typed on the
// on-screen keyboard with the key bar's modifiers. What isn't a key is left to xterm, which types it as text.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { applyModsToText, chord, fromKeyEvent } from '../web/src/terminal/keys.ts';

type Held = Partial<Pick<KeyboardEvent, 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>>;
/** A keydown with the fields fromKeyEvent reads: the key's value, its physical position and the held modifiers. */
const down = (key: string, code: string, held: Held = {}) => ({ key, code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...held }) as KeyboardEvent;

test('modifiers are written in one order, whichever was pressed first', () => {
  assert.equal(chord('x', []), 'x');
  assert.equal(chord('x', ['super', 'shift', 'alt', 'ctrl']), 'ctrl+alt+shift+super+x');
  assert.equal(chord('up', ['shift', 'ctrl']), 'ctrl+shift+up');
});

test('arrows, Escape, Tab, Backspace and function keys go by name, with their modifiers', () => {
  assert.deepEqual(fromKeyEvent(down('ArrowUp', 'ArrowUp')), { keys: ['up'] });
  assert.deepEqual(fromKeyEvent(down('ArrowLeft', 'ArrowLeft', { shiftKey: true })), { keys: ['shift+left'] });
  assert.deepEqual(fromKeyEvent(down('ArrowRight', 'ArrowRight', { ctrlKey: true, altKey: true, shiftKey: true })), { keys: ['ctrl+alt+shift+right'] });
  assert.deepEqual(fromKeyEvent(down('ArrowDown', 'ArrowDown', { altKey: true })), { keys: ['alt+down'] });
  assert.deepEqual(fromKeyEvent(down('Escape', 'Escape')), { keys: ['esc'] });
  assert.deepEqual(fromKeyEvent(down('Tab', 'Tab')), { keys: ['tab'] });
  assert.deepEqual(fromKeyEvent(down('Tab', 'Tab', { shiftKey: true })), { keys: ['shift+tab'] });
  assert.deepEqual(fromKeyEvent(down('Backspace', 'Backspace', { altKey: true })), { keys: ['alt+backspace'] });
  assert.deepEqual(fromKeyEvent(down('F1', 'F1')), { keys: ['f1'] });
  assert.deepEqual(fromKeyEvent(down('F12', 'F12', { shiftKey: true })), { keys: ['shift+f12'] });
});

test('Return is Enter, Shift+Return a line feed', () => {
  assert.deepEqual(fromKeyEvent(down('Enter', 'Enter')), { keys: ['enter'] });
  assert.deepEqual(fromKeyEvent(down('Enter', 'Enter', { shiftKey: true })), { text: '\n' });
  assert.deepEqual(fromKeyEvent(down('Enter', 'Enter', { ctrlKey: true })), { keys: ['ctrl+enter'] });
});

test('Home, End, Page Up/Down, Delete and Insert are sent as keys; with a modifier xterm writes them', () => {
  for (const [key, name] of [['Home', 'home'], ['End', 'end'], ['PageUp', 'pageup'], ['PageDown', 'pagedown'], ['Delete', 'delete'], ['Insert', 'insert']])
    assert.deepEqual(fromKeyEvent(down(key, key)), { key: name });
  assert.equal(fromKeyEvent(down('PageUp', 'PageUp', { shiftKey: true })), null);
});

test('control chords use the physical key, so they work in any layout, and ignore Shift', () => {
  assert.deepEqual(fromKeyEvent(down('c', 'KeyC', { ctrlKey: true })), { keys: ['ctrl+c'] });
  assert.deepEqual(fromKeyEvent(down('C', 'KeyC', { ctrlKey: true, shiftKey: true })), { keys: ['ctrl+c'] });
  assert.deepEqual(fromKeyEvent(down('ц', 'KeyW', { ctrlKey: true })), { keys: ['ctrl+w'] }, 'a Russian layout');
  assert.deepEqual(fromKeyEvent(down('2', 'Digit2', { ctrlKey: true })), { keys: ['ctrl+2'] });
  assert.deepEqual(fromKeyEvent(down('[', 'BracketLeft', { ctrlKey: true })), { keys: ['ctrl+['] });
  assert.deepEqual(fromKeyEvent(down('\\', 'Backslash', { ctrlKey: true })), { keys: ['ctrl+\\'] });
  assert.deepEqual(fromKeyEvent(down('r', 'KeyR', { ctrlKey: true, altKey: true })), { keys: ['ctrl+alt+r'] });
  assert.equal(fromKeyEvent(down(';', 'Semicolon', { ctrlKey: true })), null, 'no control chord: xterm types it');
});

test('Option chords are sent, except where Option types a character', () => {
  assert.deepEqual(fromKeyEvent(down('b', 'KeyB', { altKey: true })), { keys: ['alt+b'] });
  assert.equal(fromKeyEvent(down('∫', 'KeyB', { altKey: true })), null, 'Option+B on a Mac');
  assert.equal(fromKeyEvent(down('™', 'Digit2', { altKey: true })), null, 'Option+2 on a Mac');
});

test('the key bar’s modifiers join the hardware ones, once each', () => {
  assert.deepEqual(fromKeyEvent(down('ArrowUp', 'ArrowUp'), ['super']), { keys: ['super+up'] });
  assert.deepEqual(fromKeyEvent(down('ArrowUp', 'ArrowUp', { ctrlKey: true }), ['ctrl', 'shift']), { keys: ['ctrl+shift+up'] });
  assert.equal(fromKeyEvent(down('Home', 'Home'), ['ctrl']), null, 'a modifier from the bar, like a held one');
});

test('Command shortcuts and plain typing are left to the browser and xterm', () => {
  assert.equal(fromKeyEvent(down('c', 'KeyC', { metaKey: true })), null);
  assert.equal(fromKeyEvent(down('ArrowLeft', 'ArrowLeft', { metaKey: true })), null);
  assert.equal(fromKeyEvent(down('a', 'KeyA')), null);
  assert.equal(fromKeyEvent(down('A', 'KeyA', { shiftKey: true })), null);
});

test('typed text without modifiers stays text', () => {
  assert.deepEqual(applyModsToText('a', []), { text: 'a' });
  assert.deepEqual(applyModsToText('hello', []), { text: 'hello' });
});

test('Ctrl and Option from the key bar turn a typed letter, digit or bracket into a chord', () => {
  assert.deepEqual(applyModsToText('c', ['ctrl']), { keys: ['ctrl+c'] });
  assert.deepEqual(applyModsToText('C', ['ctrl']), { keys: ['ctrl+c'] });
  assert.deepEqual(applyModsToText('c', ['ctrl', 'shift']), { keys: ['ctrl+c'] });
  assert.deepEqual(applyModsToText('1', ['ctrl', 'shift']), { keys: ['ctrl+1'] });
  assert.deepEqual(applyModsToText('x', ['alt']), { keys: ['alt+x'] });
  assert.deepEqual(applyModsToText('[', ['ctrl', 'alt']), { keys: ['ctrl+alt+['] });
  assert.deepEqual(applyModsToText('@', ['ctrl']), { keys: ['ctrl+@'] });
  assert.deepEqual(applyModsToText(';', ['ctrl']), { text: ';' }, 'no control chord: the character is typed');
});

test('Command from the key bar is a chord with letters and punctuation alike', () => {
  assert.deepEqual(applyModsToText('a', ['super']), { keys: ['super+a'] });
  assert.deepEqual(applyModsToText('/', ['super']), { keys: ['super+/'] });
  assert.deepEqual(applyModsToText('.', ['super', 'alt']), { keys: ['alt+super+.'] });
  assert.deepEqual(applyModsToText('é', ['super']), { text: 'é' });
});

test('Shift from the key bar capitalizes what is typed', () => {
  assert.deepEqual(applyModsToText('a', ['shift']), { text: 'A' });
  assert.deepEqual(applyModsToText('hi', ['shift']), { text: 'HI' });
  assert.deepEqual(applyModsToText('hi', ['ctrl']), { text: 'hi' }, 'more than one character: no chord');
});
