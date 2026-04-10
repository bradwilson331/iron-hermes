"""IronHermes tool bridge -- provides agent tool access from Python scripts.

Usage:
    from hermes_tools import web_search, read_file, write_file
    result = web_search("python asyncio tutorial")
    content = read_file("/path/to/file.txt")
"""

import json
import os
import socket


class HermesRpcError(Exception):
    """Raised when an RPC call returns an error."""
    pass


class HermesCallLimitError(HermesRpcError):
    """Raised when the RPC call limit is exceeded."""
    pass


_sock = None
_request_id = 0


def _connect():
    global _sock
    if _sock is None:
        addr = os.environ.get("IRONHERMES_RPC_ADDR")
        if not addr:
            raise HermesRpcError(
                "IRONHERMES_RPC_ADDR not set -- not running in IronHermes sandbox"
            )
        _sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        _sock.connect(addr)
    return _sock


def _call(method, params):
    """Send a JSON-RPC 2.0 request and return the result."""
    global _request_id
    _request_id += 1
    s = _connect()
    req = json.dumps({
        "jsonrpc": "2.0",
        "id": _request_id,
        "method": method,
        "params": params,
    })
    s.sendall((req + "\n").encode("utf-8"))

    # Read response -- accumulate until newline (per Pitfall 4 from RESEARCH)
    buf = b""
    while b"\n" not in buf:
        chunk = s.recv(65536)
        if not chunk:
            raise IOError("RPC connection closed unexpectedly")
        buf += chunk

    line = buf.split(b"\n", 1)[0]
    resp = json.loads(line)

    if "error" in resp:
        code = resp["error"].get("code", 0)
        msg = resp["error"].get("message", "Unknown RPC error")
        if code == -32000:
            raise HermesCallLimitError(msg)
        raise HermesRpcError(msg)

    return resp.get("result", "")


# === Tool functions (D-07 allowed subset) ===


def read_file(path):
    """Read a file and return its contents."""
    return _call("read_file", {"path": path})


def write_file(path, content):
    """Write content to a file."""
    return _call("write_file", {"path": path, "content": content})


def patch(path, diff):
    """Apply a patch/diff to a file."""
    return _call("patch", {"path": path, "diff": diff})


def search_files(pattern, path="."):
    """Search for files matching a pattern."""
    return _call("search_files", {"pattern": pattern, "path": path})


def web_search(query):
    """Search the web and return results."""
    return _call("web_search", {"query": query})


def web_read(url):
    """Read content from a URL."""
    return _call("web_read", {"url": url})


def memory(action, **kwargs):
    """Interact with the agent's memory store."""
    params = {"action": action}
    params.update(kwargs)
    return _call("memory", params)
