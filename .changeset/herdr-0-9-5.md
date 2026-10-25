---
"briardr": patch
---

**`briardr setup` and the Linux helper install Herdr 0.9.5, and Briardr is tested with Herdr 0.9.3 to 0.9.5.** Setup no longer warns about a Herdr you already have when it is one of these versions. Upgrading isn't required, but newer Herdr helps Briardr: from 0.9.4, closing the terminal you started Herdr in no longer stops its agents, and Codex no longer stays unknown after a turn; 0.9.5 also shows Claude Code questions in custom-agent sessions (`claude --agent`) as needing you. From 0.9.4, the key bar's ⌘ reaches only programs that read it (those using the kitty keyboard protocol), and no longer types stray characters into plain shells. Replacing Herdr restarts the agents in it, so upgrade when none are working.
