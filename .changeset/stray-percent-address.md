---
"briardr": patch
---

**An address with a stray `%` opens Agents instead of a blank page.** Briardr couldn't decode an address
like `/stream/100%` and didn't start when it opened from its offline copy, with the host out of reach.
