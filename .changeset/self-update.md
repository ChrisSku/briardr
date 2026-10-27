---
"briardr": patch
---

**Briardr checks for new releases and updates itself.** Once a day the host asks GitHub whether a newer
release is out, and Settings → About shows it with its release notes and an **Update** button (the
About row in Settings says so too). Updating downloads the release for the host, checks it against the
release's checksums, replaces the installed folder and, when Briardr runs at login (the LaunchAgent) or
under systemd, restarts on the new version; Herdr and your agents keep running and devices reconnect.
On the host, `briardr update` does the same (`--check` only says whether there's a newer release).

A checkout, the Linux helper's included, doesn't replace itself: Settings → About and `briardr update`
show the command to run. Turn the daily check off with **Check for updates daily** in Settings → About,
or `"updates": {"check": false}` in config.json.
