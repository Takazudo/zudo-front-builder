import assert from "node:assert/strict";
import { closeSync, constants } from "node:fs";
import { open } from "node:fs/promises";
import { createHash } from "node:crypto";
const MAX_PACKET = 4096;
const CONTROL_SIZE = 56;
const SOCKET_LEVEL = 1;
const RIGHTS = 1;
const CREDENTIALS = 2;
const DONTWAIT = 0x40;
const CMSG_CLOEXEC = 0x40000000;
const NOSIGNAL = 0x4000;

// msghdr/iovec/cmsghdr and constants are tied to this exact Linux x86_64 ABI.
// Kept behind the explicit transport factory so ordinary runs never load FFI.
const libcPath = "/usr/lib/x86_64-linux-gnu/libc.so.6";
const libcDigest = "3a15d66867d83762c7f2f1e37359cb8f6c5743edb369c65285cb0b1c4f7498bf";
export async function openNextOuterReceiver({ signal, deadlineNs } = {}) {
  const current = process.hrtime.bigint();
  const localEnd = current + 2_000_000_000n;
  const end = deadlineNs !== undefined && deadlineNs < localEnd ? deadlineNs : localEnd;
  const check = () => {
    signal?.throwIfAborted();
    assert(process.hrtime.bigint() < end, "outer receiver preparation expired");
  };
  check();
  assert.equal(process.platform, "linux");
  assert.equal(process.arch, "x64");
  assert.equal(process.version, "v26.13.1");
  const ffi = await import("node:ffi");
  // Load the same descriptor whose bytes were verified. Initial hashing belongs
  // in bounded pre-launch preparation, outside the decision path.
  check();
  const file = await open(
    libcPath,
    constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
  );
  let handle;
  try {
    check();
    const before = await file.stat({ bigint: true });
    assert(before.isFile() && before.size > 0n && before.size <= 4n * 1024n * 1024n);
    const digest = createHash("sha256");
    const segment = Buffer.alloc(64 * 1024);
    for (let pos = 0; pos < Number(before.size);) {
      const want = Math.min(segment.length, Number(before.size) - pos);
      check();
      const { bytesRead } = await file.read(segment, 0, want, pos);
      check();
      assert.equal(bytesRead, want);
      digest.update(segment.subarray(0, want));
      pos += want;
    }
    const after = await file.stat({ bigint: true });
    check();
    assert(
      after.ctimeNs === before.ctimeNs &&
        after.mtimeNs === before.mtimeNs &&
        after.size === before.size,
    );
    assert.equal(digest.digest("hex"), libcDigest);
    handle = ffi.dlopen("/proc/self/fd/" + file.fd, {
      recvmsg: { arguments: ["int32", "buffer", "int32"], return: "int64" },
      send: { arguments: ["int32", "buffer", "uint64", "int32"], return: "int64" },
    });
  } finally {
    await file.close();
  }
  try {
    check();
  } catch (error) {
    handle.lib.close();
    throw error;
  }
  return {
    close() {
      handle.lib.close();
    },
    sendAuthority(fd, authority) {
      const packet = Buffer.from(JSON.stringify(authority));
      assert(packet.length > 0 && packet.length <= 8192, "outer authority cap");
      const sent = handle.functions.send(fd, packet, BigInt(packet.length), DONTWAIT | NOSIGNAL);
      assert.equal(Number(sent), packet.length, "outer authority delivery unavailable");
    },
    receive(fd, peer) {
      const body = Buffer.alloc(MAX_PACKET);
      const vec = Buffer.alloc(16);
      vec.writeBigUInt64LE(ffi.getRawPointer(body), 0);
      vec.writeBigUInt64LE(BigInt(body.byteLength), 8);
      const control = Buffer.alloc(CONTROL_SIZE);
      const message = Buffer.alloc(56);
      message.writeBigUInt64LE(ffi.getRawPointer(vec), 16);
      message.writeBigUInt64LE(1n, 24);
      message.writeBigUInt64LE(ffi.getRawPointer(control), 32);
      message.writeBigUInt64LE(BigInt(control.length), 40);
      const bytes = Number(handle.functions.recvmsg(fd, message, DONTWAIT | CMSG_CLOEXEC));
      // Keep all buffers holding native pointers rooted through the synchronous call.
      assert.equal(vec.readBigUInt64LE(0), ffi.getRawPointer(body));
      assert(bytes >= 0, "outer recvmsg failed");
      const flags = message.readInt32LE(48);
      const size = Number(message.readBigUInt64LE(40));
      assert(size >= 0 && size <= control.length, "control size");
      const credentials = [];
      const rights = [];
      let unexpected = false;
      try {
        for (let off = 0; off < size;) {
          assert(size - off >= 16, "short cmsg header");
          const length = Number(control.readBigUInt64LE(off));
          assert(length >= 16 && length <= size - off, "invalid cmsg length");
          const level = control.readInt32LE(off + 8),
            type = control.readInt32LE(off + 12);
          if (level === SOCKET_LEVEL && type === RIGHTS) {
            assert.equal((length - 16) % 4, 0, "partial fd");
            for (let n = off + 16; n < off + length; n += 4) rights.push(control.readInt32LE(n));
          } else if (level === SOCKET_LEVEL && type === CREDENTIALS && length === 28) {
            credentials.push({
              pid: control.readInt32LE(off + 16),
              uid: control.readUInt32LE(off + 20),
              gid: control.readUInt32LE(off + 24),
            });
          } else unexpected = true;
          off += Math.ceil(length / 8) * 8;
        }
        // A zero-byte AF_UNIX packet may still have installed SCM_RIGHTS.
        // Parse the kernel's ancillary vector before rejecting any payload.
        assert(bytes > 0 && bytes <= body.length, "missing or invalid datagram");
        assert(
          !(flags & (32 | 8)) && !unexpected && !rights.length,
          "unexpected/truncated ancillary data",
        );
        assert.deepEqual(credentials, [peer], "wrong kernel sender");
        return {
          payload: JSON.parse(body.toString("utf8", 0, bytes)),
          peer: credentials[0],
          receivedNs: String(process.hrtime.bigint()),
        };
      } finally {
        for (const right of rights) closeSync(right);
      }
    },
  };
}
