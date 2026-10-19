// The plugin's client: what it adds to Briardr. The IDs match "contributes" in briardr-plugin.json.
import { definePlugin } from '@briardr/sdk';
import Settings from './Settings.svelte';

export default definePlugin({ settings: { {{tab}}: Settings } });
