---
"briardr": patch
---

**`briardr revoke` signs a device out while Briardr is running.** Until now a lost device revoked with
`briardr revoke` on the host stayed signed in until Briardr restarted, and Briardr could write it back
into its devices file a minute later, undoing the revoke. Now it works like revoking in Settings →
Devices: the device's next request is refused, its open pages disconnect within a second, and its alerts
stop. If you revoked a device with `briardr revoke` while Briardr was running, check `briardr devices`
and revoke it again if it is still listed.
