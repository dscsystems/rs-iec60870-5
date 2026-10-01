"""Connect two raw pseudo terminals to exercise both real serial backends."""
import json
import os
import pty
import select
import tty

left, left_slave = pty.openpty()
right, right_slave = pty.openpty()
tty.setraw(left_slave)
tty.setraw(right_slave)
print(json.dumps({"event": "ready", "addr": os.ttyname(left_slave), "other": os.ttyname(right_slave)}), flush=True)
while True:
    ready, _, _ = select.select([left, right], [], [])
    for fd in ready:
        data = os.read(fd, 4096)
        destination = right if fd == left else left
        while data:
            data = data[os.write(destination, data):]
