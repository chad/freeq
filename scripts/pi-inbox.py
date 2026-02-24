#!/usr/bin/env python3
import json
import os
import time

OUTBOX = os.environ.get("PI_OUTBOX", "/tmp/freeq-pi-queue.jsonl")
STATE = os.environ.get("PI_INBOX_STATE", "/tmp/freeq-pi-queue.offset")
POLL = float(os.environ.get("PI_INBOX_POLL", "0.5"))


def load_offset() -> int:
    try:
        with open(STATE, "r", encoding="utf-8") as f:
            return int(f.read().strip() or "0")
    except FileNotFoundError:
        return 0
    except Exception:
        return 0


def save_offset(offset: int) -> None:
    tmp = f"{STATE}.tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(str(offset))
    os.replace(tmp, STATE)


def main() -> None:
    offset = load_offset()
    while True:
        try:
            with open(OUTBOX, "a+", encoding="utf-8") as f:
                f.seek(0, os.SEEK_END)
                end = f.tell()
                if end < offset:
                    offset = 0
                f.seek(offset)
                data = f.read()
                if data:
                    for line in data.splitlines():
                        if not line.strip():
                            continue
                        try:
                            entry = json.loads(line)
                        except json.JSONDecodeError:
                            continue
                        ts = entry.get("ts")
                        did = entry.get("did")
                        text = entry.get("text")
                        target = entry.get("target")
                        print(f"[pi:{target}] {text} (did={did}, ts={ts})", flush=True)
                offset = f.tell()
                save_offset(offset)
        except FileNotFoundError:
            pass
        time.sleep(POLL)


if __name__ == "__main__":
    main()
