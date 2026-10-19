import { definePlugin } from '@briardr/sdk';
import ChangesSurface from './ChangesSurface.svelte';

export default definePlugin({ streamTabs: { changes: ChangesSurface } });
