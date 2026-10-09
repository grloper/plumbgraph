#!/usr/bin/env python3
"""Tiny fake LSP server for plumbgraph tests: reports two diagnostics per opened file.

  --slow         like rust-analyzer: empty diagnostics first, a long work-done-progress phase,
                 real diagnostics only afterwards
  --slow-status  same, but loading is signalled with experimental/serverStatus (quiescent)
Like rust-analyzer, the loading state is only reported to clients that advertise support.
"""
import json
import sys
import threading
import time

SLOW = "--slow" in sys.argv or "--slow-status" in sys.argv
STATUS = "--slow-status" in sys.argv
LOCK = threading.Lock()
CAPS = {}


def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode().strip()
        if not line:
            break
        k, v = line.split(":", 1)
        headers[k.lower()] = v.strip()
    n = int(headers["content-length"])
    return json.loads(sys.stdin.buffer.read(n))


def send(m):
    b = json.dumps(m).encode()
    with LOCK:
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
        sys.stdout.buffer.flush()


def diagnostics(uri, lang):
    return {"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
        "uri": uri,
        "diagnostics": [
            {"range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 9}},
             "severity": 1, "code": "E1", "source": "fake", "message": "fake problem\x1b[31m in " + lang},
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
             "severity": 4, "message": "just a hint"},
        ]}}


def slow_workspace_load(uri, lang):
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
          "params": {"uri": uri, "diagnostics": []}})
    if STATUS:
        if not CAPS.get("experimental", {}).get("serverStatusNotification"):
            return
        send({"jsonrpc": "2.0", "method": "experimental/serverStatus",
              "params": {"quiescent": False, "health": "ok"}})
        time.sleep(2.2)
        send({"jsonrpc": "2.0", "method": "experimental/serverStatus",
              "params": {"quiescent": True, "health": "ok"}})
    else:
        if not CAPS.get("window", {}).get("workDoneProgress"):
            return
        send({"jsonrpc": "2.0", "method": "$/progress",
              "params": {"token": "t", "value": {"kind": "begin", "title": "Loading"}}})
        time.sleep(2.2)
        send({"jsonrpc": "2.0", "method": "$/progress",
              "params": {"token": "t", "value": {"kind": "end"}}})
    send(diagnostics(uri, lang))


while True:
    m = read()
    if m is None:
        break
    meth = m.get("method")
    if meth == "initialize":
        CAPS = m["params"].get("capabilities", {})
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"capabilities": {"textDocumentSync": 1}}})
    elif meth == "initialized":
        # server -> client request the client must answer
        send({"jsonrpc": "2.0", "id": 900, "method": "window/workDoneProgress/create", "params": {"token": "t"}})
    elif meth == "textDocument/didOpen":
        td = m["params"]["textDocument"]
        if SLOW:
            threading.Thread(target=slow_workspace_load, args=(td["uri"], td["languageId"]), daemon=True).start()
        else:
            send(diagnostics(td["uri"], td["languageId"]))
    elif meth == "shutdown":
        send({"jsonrpc": "2.0", "id": m["id"], "result": None})
    elif meth == "exit":
        break
