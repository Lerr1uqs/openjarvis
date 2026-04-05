#!/usr/bin/env python3
"""Interactive command-session fixture for background and TTY tests."""

import sys


def prompt(message: str) -> str:
    print(message, flush=True)
    line = sys.stdin.readline()
    if not line:
        raise EOFError("stdin closed")
    return line.strip()


def main() -> int:
    first = int(prompt("FIRST?"))
    print(f"FIRST={first}", flush=True)
    second = int(prompt("SECOND?"))
    print(f"SECOND={second}", flush=True)
    total = int(prompt("SUM?"))
    if total == first + second:
        print("OK", flush=True)
        return 0

    print("FAIL", flush=True)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
