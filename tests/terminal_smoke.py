#!/usr/bin/env python3
"""Linux 伪终端集成检查：不使用 DISPLAY，不创建图形窗口。

先 cargo build，再 python3 tests/terminal_smoke.py [可执行文件路径]。
仅依赖 Python 标准库；所有输入都发送到本脚本创建的子进程 PTY。
"""
import codecs
import errno
import fcntl
import os
import pty
import re
import select
import statistics
import struct
import subprocess
import sys
import termios
import time

BINARY = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug/animation_eyes")
BLOCKS = set("▘▝▀▖▌▞▛▗▚▐▜▄▙▟█")


class Screen:
    def __init__(self, width, height):
        self.decoder = codecs.getincrementaldecoder("utf-8")()
        self.pending = ""
        self.color = None
        self.clears = 0
        self.resize(width, height)

    def resize(self, width, height):
        self.width, self.height = width, height
        self.cells = [[" "] * width for _ in range(height)]
        self.colors = [[None] * width for _ in range(height)]
        self.x = self.y = 0

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending.startswith("\x1b["):
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", self.pending)
                if not match:
                    return
                params, _, command = match.groups()
                self.pending = self.pending[match.end():]
                if command in "Hf":
                    values = [int(part or 1) for part in params.split(";")]
                    row, column = (values + [1, 1])[:2]
                    assert 1 <= row <= self.height and 1 <= column <= self.width
                    self.x, self.y = column - 1, row - 1
                elif command == "J" and params == "2":
                    self.cells = [[" "] * self.width for _ in range(self.height)]
                    self.colors = [[None] * self.width for _ in range(self.height)]
                    self.clears += 1
                elif command == "m":
                    if params.startswith("38;2;"):
                        self.color = tuple(map(int, params.split(";")[2:5]))
                    elif params == "38;5;15":
                        self.color = (255, 255, 255)
                    elif params in ("0", "39", ""):
                        self.color = None
                continue
            if self.pending == "\x1b":
                return
            char, self.pending = self.pending[0], self.pending[1:]
            assert char == " " or char == "o" or char in BLOCKS or "\u2801" <= char <= "\u28ff", repr(char)
            assert 0 <= self.x < self.width and 0 <= self.y < self.height
            self.cells[self.y][self.x] = char
            self.colors[self.y][self.x] = self.color
            self.x += 1

    def text(self):
        return "\n".join("".join(row) for row in self.cells)

    def warmth(self):
        values = [color[0] - color[2]
                  for chars, colors in zip(self.cells, self.colors)
                  for char, color in zip(chars, colors) if char != " " and color]
        assert values
        return statistics.median(values)


class Session:
    def __init__(self, *arguments, width=100, height=32, parse_screen=True):
        self.master, self.slave = pty.openpty()
        self.before = termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        env = {key: value for key, value in os.environ.items()
               if key not in ("DISPLAY", "WAYLAND_DISPLAY", "NO_COLOR")}
        env["TERM"] = "xterm-256color"

        def setup():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen([BINARY, *arguments], stdin=self.slave,
                                        stdout=self.slave, stderr=self.slave,
                                        preexec_fn=setup, env=env)
        self.screen = Screen(width, height) if parse_screen else None
        self.output = bytearray()
        self.closed = False

    def read(self, duration=0.1):
        received = bytearray()
        end = time.monotonic() + duration
        while time.monotonic() < end:
            if not select.select([self.master], [], [], max(0, end - time.monotonic()))[0]:
                break
            try:
                chunk = os.read(self.master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not chunk:
                break
            self.output.extend(chunk)
            received.extend(chunk)
            if self.screen is not None:
                self.screen.feed(chunk)
        return bytes(received)

    def send(self, data, duration=0.1):
        os.write(self.master, data)
        return self.read(duration)

    def resize(self, width, height):
        self.read(0.02)
        self.screen.resize(width, height)
        clears = self.screen.clears
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        self.read(0.15)
        assert self.process.poll() is None
        assert self.screen.clears > clears

    def finish(self, key=b"q", mouse=True):
        if key is not None:
            self.send(key)
        assert self.process.wait(timeout=5) == 0
        self.read(0.02)
        assert termios.tcgetattr(self.slave) == self.before
        sequences = [b"\x1b[?25h", b"\x1b[?7h", b"\x1b[?1049l", b"\x1b[?1004l"]
        if mouse:
            sequences.append(b"\x1b[?1003l")
        for sequence in sequences:
            assert sequence in self.output, sequence
        self.abort()

    def abort(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        if not self.closed:
            os.close(self.master)
            os.close(self.slave)
            self.closed = True


def main():
    session = Session()
    try:
        session.read(0.15)
        assert b"\x1b[?1003h" in session.output
        assert b"\x1b[?1004h" in session.output
        session.send(b"p")
        frozen = session.screen.text()
        assert not session.read(0.3)
        assert session.screen.text() == frozen
        clears = session.screen.clears
        session.send(b"b")
        assert any(char in BLOCKS for char in session.screen.text())
        session.send(b"b")
        assert not any(char in BLOCKS for char in session.screen.text())
        session.send(b"c")
        session.send(b"c")
        assert session.screen.clears == clears
        print("通过：无 DISPLAY 运行，只显示眼睛；暂停无输出，显示/颜色切换不清屏。")

        for size in [(240, 8), (20, 80), (12, 4), (3, 2), (1, 1), (100, 32)]:
            session.resize(*size)
        print("通过：宽矮、窄高、1×1 和恢复尺寸，所有光标与输出均不越界。")

        session.send(b"p")
        session.read(0.2)
        cool = session.screen.warmth()
        session.send(b"\r", duration=2.2)
        assert session.screen.warmth() > cool + 8
        before = session.screen.text()
        session.send(b" ", duration=0.45)
        assert session.screen.text() != before
        session.send(b"\x1b[C", duration=0.3)
        session.send(b"\x1b[<0;31;17M", duration=0.2)
        for column in [29, 31, 33, 31, 29]:
            session.send(f"\x1b[<32;{column};17M".encode(), duration=0.08)
        session.send(b"\x1b[<0;29;17m")
        session.send(b"\x1b[O", duration=1.2)
        assert session.process.poll() is None
        session.send(b"\x1b[I", duration=0.5)
        session.send(b"m")
        session.send(b"m")
        assert session.screen.clears == clears + 6
        session.finish()
        print("通过：键盘安抚与招呼、方向键、SGR 鼠标抚摸、焦点重逢及退出恢复。")
    finally:
        session.abort()

    for key in [b"Q", b"\x1b", b"\x03"]:
        session = Session()
        try:
            session.read()
            session.finish(key)
        finally:
            session.abort()

    session = Session("--no-mouse", "--mono", "--reduced-motion", "--frames", "12")
    try:
        session.read(0.8)
        session.finish(key=None, mouse=False)
        assert b"\x1b[?1003h" not in session.output
    finally:
        session.abort()
    print("通过：Q/Esc/Ctrl+C、无鼠标模式、减少动态、有限帧退出。")

    # 超大 PTY 尺寸应在分配前拒绝，并且错误退出也要完整恢复终端。
    session = Session(width=65535, height=65535, parse_screen=False)
    try:
        session.read(0.3)
        assert session.process.wait(timeout=3) != 0
        assert termios.tcgetattr(session.slave) == session.before
        assert "终端尺寸超过安全绘制范围".encode() in session.output
        for sequence in [b"\x1b[?1003l", b"\x1b[?1004l", b"\x1b[?25h", b"\x1b[?1049l"]:
            assert sequence in session.output
    finally:
        session.abort()
    print("通过：异常超大终端在分配前被拒绝，错误退出仍恢复鼠标、焦点、光标和 raw mode。")

    help_result = subprocess.run([BINARY, "--help"], capture_output=True)
    assert help_result.returncode == 0
    non_tty = subprocess.run([BINARY], input=b"", capture_output=True)
    assert non_tty.returncode != 0 and not non_tty.stdout
    assert "交互式终端".encode() in non_tty.stderr
    print("通过：帮助不需要 TTY，非交互环境明确报错，不启动图形窗口。")


if __name__ == "__main__":
    main()
