---
"briardr": patch
---

**Changes: comment on lines of a diff and send the comments to the agent.** Tap a line number in a diff (a second number makes a range), choose Comment and say what should change. The comments stay on this device, for each stream, until you send or clear them, and their lines stay marked while the file is unchanged. Review shows them with an optional note and sends them to the terminal as one paste; Briardr never presses Enter. Plugins get the same prompt sheet that Files and Changes use: `PromptSheet` and `codeBlock` in `@briardr/sdk`.
