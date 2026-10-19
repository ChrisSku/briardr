#!/usr/bin/env python3
# Test fixture: a raw-mode program that enables application cursor keys, bracketed paste and SGR
# mouse reporting (like a full-screen TUI), then prints the exact bytes it receives. Ctrl-] quits.
import os, sys, termios, tty

fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
tty.setraw(fd)
sys.stdout.write('\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006hREADY\r\n')
sys.stdout.flush()
try:
    while True:
        b = os.read(fd, 4096)
        sys.stdout.write('BYTES ' + b.hex() + '\r\n')
        sys.stdout.flush()
        if b'\x1d' in b:
            break
finally:
    sys.stdout.write('\x1b[?1l\x1b[?2004l\x1b[?1000l\x1b[?1006l')
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
