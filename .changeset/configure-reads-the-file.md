---
"briardr": patch
---

**`briardr configure` changes only the settings it's given, and refuses a config.json it can't read.**
It used to save the `BRIARDR_*` overrides of the shell it ran in (port, preview port, listeners, Herdr
session) into config.json along with the change, and to start again from the defaults when config.json
couldn't be parsed. Now it changes the file's own settings, like a change in the app's Settings, and a
config.json it can't parse stays as it is: the command says which file and why, and changes nothing.
