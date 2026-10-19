import { definePlugin } from '@briardr/sdk';
import NotesSurface from './NotesSurface.svelte';

export default definePlugin({ streamTabs: { notes: NotesSurface } });
