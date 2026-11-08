"""Wait for durable host launch intent before executing the owned runtime client."""
import json
import os


def main():
    # Read only the host command line, leaving the container's gate/stdin untouched.
    line = bytearray()
    while True:
        byte = os.read(0, 1)
        if not byte:
            raise SystemExit(125)
        if byte == b'\n':
            break
        line.extend(byte)
        if len(line) > 1024 * 1024:
            raise SystemExit(125)
    command = json.loads(line)
    os.execv(command[0], command)


if __name__ == '__main__':
    main()
