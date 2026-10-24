---
"briardr": patch
---

**A page reloads once for an update, not twice in a row.** When a new build arrived, a page you weren't
using could hear about it from both the reconnect and the service worker, a moment apart, and start its
reload over.
