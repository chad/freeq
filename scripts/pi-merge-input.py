#!/usr/bin/env python3
import os
import selectors
import sys

if len(sys.argv) < 2:
    print("usage: pi-merge-input.py <fifo>", file=sys.stderr)
    sys.exit(1)

fifo = sys.argv[1]

# Open FIFO for reading (non-blocking)
fd_fifo = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
file_fifo = os.fdopen(fd_fifo, "r", buffering=1)

# stdin
fd_stdin = sys.stdin.fileno()
os.set_blocking(fd_stdin, False)

sel = selectors.DefaultSelector()
sel.register(fd_stdin, selectors.EVENT_READ, "stdin")
sel.register(fd_fifo, selectors.EVENT_READ, "fifo")

while True:
    for key, _ in sel.select():
        if key.data == "stdin":
            data = sys.stdin.read()
            if data:
                sys.stdout.write(data)
                sys.stdout.flush()
        else:
            line = file_fifo.readline()
            if line:
                sys.stdout.write(line)
                if not line.endswith("\n"):
                    sys.stdout.write("\n")
                sys.stdout.flush()
