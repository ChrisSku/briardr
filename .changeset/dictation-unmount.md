---
"briardr": patch
---

**Dictation stops when the terminal or its Enter key goes away.** Leaving the stream, or the key bar hiding
for a hardware keyboard, while Enter was held kept the microphone recording with nothing left to end it.
Now the recording is dropped and the microphone let go of, and nothing is transcribed or inserted. The
same goes for letting go of Enter before the microphone is on (while the browser asks for it), and a
transcription still on its way when you leave the stream is no longer inserted or reported.
