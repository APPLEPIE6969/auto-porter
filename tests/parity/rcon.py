"""Minimal RCON client (Bukkit protocol) for the Java oracle side."""
import socket, struct

class Rcon:
    def __init__(self, host="127.0.0.1", port=25575, password="auto-porter", timeout=10):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.rid = 0
        self._send(3, password)
        t, _, payload = self._recv()
        if t != 2:
            raise RuntimeError("RCON auth failed")

    def _send(self, t, payload: str):
        self.rid += 1
        data = struct.pack("<ii", self.rid, t) + payload.encode() + b"\x00\x00"
        self.sock.sendall(struct.pack("<i", len(data)) + data)

    def _recv(self):
        raw = b""
        while len(raw) < 4:
            raw += self.sock.recv(4 - len(raw))
        (length,) = struct.unpack("<i", raw)
        body = b""
        while len(body) < length:
            body += self.sock.recv(length - len(body))
        rid, t = struct.unpack("<ii", body[:8])
        return t, rid, body[8:-2].decode(errors="replace")

    def command(self, cmd: str) -> str:
        self._send(2, cmd)
        t, _, payload = self._recv()
        return payload

    def close(self):
        self.sock.close()
