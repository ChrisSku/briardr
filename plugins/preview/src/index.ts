import { definePlugin, isLocalhostUrl } from '@briardr/sdk';
import PreviewSurface from './PreviewSurface.svelte';
import { previewRequests } from './requests.ts';

export default definePlugin({
  streamTabs: { preview: PreviewSurface },
  // Localhost links open in Preview, through Briardr's connection to the host.
  onTerminalLink(url, stream, briardr) {
    if (!isLocalhostUrl(url)) return false;
    previewRequests.value = { ...previewRequests.value, [stream.id]: url };
    briardr.openTab('preview');
    return true;
  },
});
