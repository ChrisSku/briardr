#!/usr/bin/env python3
# Run a Herdr TUI client for the test run's Herdr session inside a pty, as a stand-in for the Mac's own view.
import fcntl, os, pty, struct, sys, termios, time, select, signal
cols = int(sys.argv[1]) if len(sys.argv) > 1 else 140
rows = int(sys.argv[2]) if len(sys.argv) > 2 else 45
session = sys.argv[3] if len(sys.argv) > 3 else 'briardr-dev'
pid, fd = pty.fork()
if pid == 0:
    os.execvp('herdr', ['herdr', '--session', session])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
os.kill(pid, signal.SIGWINCH)
ctl = os.environ.get('TUI_CTL')  # optional fifo for resize commands "cols rows"
while True:
    r, _, _ = select.select([fd], [], [], 1.0)
    if fd in r:
        try:
            os.read(fd, 65536)
        except OSError:
            break
    if ctl and os.path.exists(ctl):
        with open(ctl) as f:
            c, rr = f.read().split()
        os.remove(ctl)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', int(rr), int(c), 0, 0))
        os.kill(pid, signal.SIGWINCH)
    try:
        wp, _ = os.waitpid(pid, os.WNOHANG)
        if wp:
            break
    except ChildProcessError:
        break
