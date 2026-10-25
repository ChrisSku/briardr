#!/usr/bin/env python3
# Test fixture: a raw-mode program that enables application cursor keys, bracketed paste and SGR
# mouse reporting (like a full-screen TUI), then prints the exact bytes it receives. Ctrl-] quits.
# --kitty also turns on the kitty keyboard protocol (disambiguate), as programs that read Command (super)
# chords do: Herdr 0.9.4 and newer don't send those to a program that turned on no keyboard protocol.
import os, sys, termios, tty

kitty = '--kitty' in sys.argv[1:]

fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
tty.setraw(fd)
sys.stdout.write('\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006h' + ('\x1b[>1u' if kitty else '') + 'READY\r\n')
sys.stdout.flush()
try:
    while True:
        b = os.read(fd, 4096)
        sys.stdout.write('BYTES ' + b.hex() + '\r\n')
        sys.stdout.flush()
        if b'\x1d' in b or b'\x1b[93;5u' in b:  # Ctrl-], as a byte or as the kitty protocol sends it
            break
finally:
    sys.stdout.write(('\x1b[<u' if kitty else '') + '\x1b[?1l\x1b[?2004l\x1b[?1000l\x1b[?1006l')
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
