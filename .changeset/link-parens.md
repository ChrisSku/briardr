---
"briardr": patch
---

**A link in the terminal keeps a closing parenthesis that matches one in it, like
`https://en.wikipedia.org/wiki/Rust_(programming_language)`.** It was opened without the `)`. A parenthesis
that closes the text around a link, as in `(see https://example.com)`, is still left out, and square brackets
and braces follow the same rule, so `http://[::1]` opens in Preview too.
