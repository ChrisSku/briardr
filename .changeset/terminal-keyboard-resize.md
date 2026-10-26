---
"briardr": patch
---

**The on-screen keyboard resizes the terminal instead of pushing its top out of view.** While you type
in a stream you control, the pane takes the rows above the keyboard, so a full-screen program such as
Claude Code lays itself out to fit: its input box and what's above it stay in view. The pane gets its
full size back when the keyboard closes; the program redraws each time. Watching never resizes the pane.
