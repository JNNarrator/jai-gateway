#!/usr/bin/env python3
"""M8 性能基线用 mock 上游：OpenAI 兼容 /v1/chat/completions，SSE 恒速流。

用法：python3 sse_mock_upstream.py [port] [bytes_per_sec]
默认 127.0.0.1:13999，1MB/s。请求体被忽略，按恒定速率推 SSE delta，
客户端断开或到达 max_seconds（默认 660s）即结束。
"""
import http.server
import json
import sys
import time

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 13999
RATE = int(sys.argv[2]) if len(sys.argv) > 2 else 1_000_000  # bytes/s
MAX_SECONDS = 660

FILLER = ("基准性能测试数据流。" * 171)[:4096]  # ~4KB UTF-8 中文载荷


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        print(f"[mock] POST {self.path} content-length={length}", flush=True)
        self.rfile.read(length)
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        start = time.monotonic()
        sent = 0
        try:
            while time.monotonic() - start < MAX_SECONDS:
                target = int((time.monotonic() - start) * RATE)
                if target - sent < 4096:  # 未落后一个 chunk 就等待
                    time.sleep(0.002)
                    continue
                event = (
                    "data: "
                    + json.dumps(
                        {
                            "id": "chatcmpl-bench",
                            "object": "chat.completion.chunk",
                            "choices": [{"index": 0, "delta": {"content": FILLER}}],
                        },
                        ensure_ascii=False,
                    )
                    + "\n\n"
                ).encode()
                self.wfile.write(f"{len(event):x}\r\n".encode() + event + b"\r\n")
                self.wfile.flush()
                sent += len(event)
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            try:
                self.wfile.write(b"0\r\n\r\n")
            except Exception:
                pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    print(f"mock upstream on :{PORT} rate={RATE}B/s", flush=True)
    http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
