#!/usr/bin/env python3
# search-probe.py TEXT : ask the running atlas-search-service (Unix socket) and print matching paths.
# Run inside the VM (or any Linux session where Atlas runs): it shows what the index finds, without the UI.
import json, os, socket, sys
sock = os.path.join(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"), "atlas-search.sock")
q = {"Search": {"generation": 1, "text": sys.argv[1], "interactive": False, "scope": "Everything",
     "filters": {"categories": [], "extensions": [], "min_size": None, "max_size": None,
                 "modified_after": None, "modified_before": None},
     "sort": {"field": "Relevance", "descending": False}, "offset": 0, "limit": 50}}
s = socket.socket(socket.AF_UNIX); s.connect(sock); s.sendall((json.dumps(q) + "\n").encode())
buf = b""
while not buf.endswith(b"\n"):
    c = s.recv(65536)
    if not c: break
    buf += c
r = json.loads(buf)
if "Search" in r:
    for item in r["Search"]["results"]: print(item["path"])
else: print(r, file=sys.stderr); sys.exit(1)
