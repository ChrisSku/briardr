---
"briardr": patch
---

**Briardr works with the Herdr desktop app too, through its herdr-core.** Choose **Herdr app** in
Settings → Herdr (or `briardr configure --herdr-backend core`); **Herdr TUI** stays the default and
switches back. With the Herdr app you watch, peek and type in its terminals, and Briardr encodes your
keys, pastes and clicks the way the app's own terminal does. You rename, create and close its spaces and
tabs as workspaces and projects. Opening a finished agent marks it seen in the app too, and terminals
on machines the app added show up without their file tools. Briardr never starts a herdr-core: open the
Herdr app first. Plugin stream tabs can say `"local": true` to hide themselves for terminals on another
machine.
