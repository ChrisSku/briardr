---
"briardr": patch
---

**Docs and Notes now render the HTML in Markdown files, like the centred header of a README.** Common tags such as `<p align="center">`, `<img>`, `<a>`, `<details>` and `<kbd>` work, in the same way as on GitHub. Anything that could run or reach out stays out: scripts, event handlers, styles and forms never get through, other tags show as text, and remote images still aren't loaded.
