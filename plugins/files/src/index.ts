import { definePlugin } from '@briardr/sdk';
import FilesSurface from './FilesSurface.svelte';

export default definePlugin({ streamTabs: { files: FilesSurface } });
