// A localhost link tapped in a stream's terminal, waiting for that stream's Preview tab to open it.
import { state } from '@briardr/sdk';

export const previewRequests = state<Record<string, string>>({});

export function consumePreviewRequest(streamId: string) {
  const { [streamId]: _, ...rest } = previewRequests.value;
  previewRequests.value = rest;
}
