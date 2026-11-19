---
"briardr": patch
---

**Plugins get Briardr's own messages typed: `onCoreEvent` knows what each one carries.** The client's
types for the API and the live and terminal sockets are now generated from the server's, and the
`@briardr/sdk` types plugins use come from them: `briardr.onCoreEvent('notes', (m) => …)` types `m` as
`{ t, streamId, noteId, revision }` (any other type still works as before), and `Settings.notifications`
also has `debounceMs`. Nothing changes at run time.
