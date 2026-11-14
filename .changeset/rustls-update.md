---
"briardr": patch
---

**rustls, the TLS library, is updated to 0.23.46.** It fixes four advisories (one moderate, three low) in how rustls handles a rejected Encrypted Client Hello, an unauthenticated ServerHello, the debug output of sessions, and compressed certificates. Briardr's own HTTPS listener and its requests to Web Push and usage services use rustls.
