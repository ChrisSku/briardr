import { definePlugin } from '@briardr/sdk';
import DocsSurface from './DocsSurface.svelte';

export default definePlugin({ streamTabs: { docs: DocsSurface } });
