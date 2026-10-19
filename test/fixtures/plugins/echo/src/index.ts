import { definePlugin } from '@briardr/sdk';
import Tab from './Tab.svelte';

export default definePlugin({ streamTabs: { echo: Tab } });
