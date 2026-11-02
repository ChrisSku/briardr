---
"briardr": patch
---

**Docs and Notes now render the HTML in Markdown files, like the centred header of a README, and draw Mermaid diagrams.** Common tags such as `<p align="center">`, `<img>`, `<a>`, `<details>` and `<kbd>` work, in the same way as on GitHub, and a ` ```mermaid ` block becomes a diagram in the current theme. Anything that could run or reach out stays out: scripts, event handlers, styles and forms never get through, other tags show as text, remote images still aren't loaded, and a diagram can't change its security settings or link anywhere. A diagram Mermaid can't draw stays as code.
