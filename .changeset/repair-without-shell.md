---
"briardr": patch
---

**Turning notifications on or repairing them no longer hangs when Briardr isn't saved on the device.**
Notifications need the offline app's service worker; when it's still installing, the button now waits up
to 10 seconds, and when it isn't installed it says so (with Settings → Diagnostics → Repair offline app),
instead of staying disabled with nothing happening.
