import { definePlugin } from '@briardr/sdk';
import UsageScreen from './UsageScreen.svelte';
import UsageSection from './UsageSection.svelte';

export default definePlugin({ appTabs: { usage: UsageScreen }, settings: { usage: UsageSection } });
