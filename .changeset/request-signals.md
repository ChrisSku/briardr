---
"briardr": patch
---

**A plugin request whose signal was aborted before the call is cancelled at once, and requests no longer
keep a listener on their signal after they end.** `briardr.api`, `briardr.core` and `briardr.upload` used
to send a request anyway when given a signal that was already aborted, and left a listener on the signal
for every request, so one signal kept for a whole screen collected them. Plugins also get
`errorMessage(e, fallback)` from `@briardr/sdk`: a failed request's message, ready to show, or the
fallback for anything else thrown.
