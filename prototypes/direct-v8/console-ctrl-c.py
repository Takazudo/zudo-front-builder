#!/usr/bin/env python3
"""Windows-only: run a command on this process's console and deliver a real Ctrl-C on request.

check-cli-lifecycle.py starts this helper with CREATE_NEW_CONSOLE, so the console
it signals holds only this helper and its child, never the observer or the CI
runner. A Ctrl-C cannot target a process group (Windows disables it in groups
created with CREATE_NEW_PROCESS_GROUP), so a private console is the only way to
deliver the event `zfb dev` handles (`tokio::signal::ctrl_c`).

A line on stdin (or EOF) requests the Ctrl-C. The helper exits with the child's
exit code, and exits as soon as the child does, so the observer sees a crash.
"""
import subprocess
import sys
import threading

CTRL_C_EVENT = 0


def exit_with(code):
    sys.stdout.flush()
    sys.stderr.flush()
    if sys.platform == "win32":
        import ctypes
        ctypes.windll.kernel32.ExitProcess(ctypes.c_uint(code & 0xFFFFFFFF))
    raise SystemExit(code)


def deliver_ctrl_c(child):
    if sys.platform != "win32":
        child.kill()
        raise SystemExit("console-ctrl-c.py: a console Ctrl-C can only be delivered on Windows")
    import ctypes
    kernel32 = ctypes.windll.kernel32
    # The event reaches every process on this console; this helper ignores it.
    kernel32.SetConsoleCtrlHandler(None, True)
    if kernel32.GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0):
        print("console-ctrl-c: CTRL_C_EVENT delivered", file=sys.stderr, flush=True)
    else:
        print(f"console-ctrl-c: GenerateConsoleCtrlEvent failed: {ctypes.GetLastError()}", file=sys.stderr)
        child.kill()


def accept_ctrl_c():
    # The "ignore Ctrl-C" attribute is inherited at process creation, and a CI
    # runner's process chain may carry it; clear it so the child inherits a
    # console on which Ctrl-C reaches its handler.
    if sys.platform == "win32":
        import ctypes
        ctypes.windll.kernel32.SetConsoleCtrlHandler(None, False)


def main(command):
    if not command:
        print("usage: console-ctrl-c.py <command> [args...]", file=sys.stderr)
        return 2
    accept_ctrl_c()
    child = subprocess.Popen(command, stdin=subprocess.DEVNULL)
    requested = threading.Event()

    def watch_stdin():
        sys.stdin.readline()
        requested.set()

    threading.Thread(target=watch_stdin, daemon=True).start()
    while child.poll() is None and not requested.wait(0.05):
        pass
    if child.poll() is None:
        deliver_ctrl_c(child)
    code = child.wait()
    print(f"console-ctrl-c: child exited {code}", file=sys.stderr, flush=True)
    return code


if __name__ == "__main__":
    exit_with(main(sys.argv[1:]))
