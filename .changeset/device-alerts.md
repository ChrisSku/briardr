---
"briardr": patch
---

**Each device chooses its own alerts, and can have quiet hours.** In Settings → Notifications, the switches for Needs you, Finished, Ready for input, low battery and Include names now apply to the device you set them on; before, turning one off on a phone turned it off for every device. A device that never chose follows the host's `notifications` setting, so nothing changes until you touch a switch, and the host keeps a device's choices until it is revoked, through turning alerts off and on and repairs. "Quiet hours" keeps a span of the device's day free of alerts (22:00 to 07:00 unless you change it), on the device's own clock, summer time included; alerts in them aren't sent later. A device revoked with `briardr revoke` while Briardr wasn't running no longer gets alerts once Briardr starts again.
