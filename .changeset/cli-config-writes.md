---
"briardr": patch
---

**A setting changed in the app no longer undoes what `briardr configure` saved before the restart.** Briardr used to save its whole running configuration on any change in Settings, so a `briardr configure` change waiting for the restart (such as `--allowed-networks`) was lost. Now a change from the app is made to `config.json` as it is on disk, and only to its own setting; the two take turns with a lock (`config.lock` in the data directory), and the restart applies both. Briardr also no longer writes its `BRIARDR_*` environment overrides into `config.json`, and won't save over a `config.json` it can't read. Plugins turned on or off with `briardr plugin enable|disable` are now noticed even when Linux gives the file the same modification time as the server's last save, and a devices file that can't be read at startup no longer makes Briardr forget every device's alert choices.
