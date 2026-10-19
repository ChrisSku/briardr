// The plugin's client: what it adds to Briardr. The IDs match "contributes" in briardr-plugin.json.
import { definePlugin } from '@briardr/sdk';
import Screen from './Screen.svelte';

export default definePlugin({ appTabs: { {{tab}}: Screen } });
