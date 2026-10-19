// The plugin's client: what it adds to Briardr. The IDs match "contributes" in briardr-plugin.json.
import { definePlugin } from '@briardr/sdk';
import Tab from './Tab.svelte';

export default definePlugin({ streamTabs: { {{tab}}: Tab } });
