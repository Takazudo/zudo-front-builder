import assert from "node:assert/strict";
import { closeSync, fstatSync, openSync, readSync, constants } from "node:fs";
import { createHash } from "node:crypto";
const MAX_PACKET = 4096;
const CONTROL_SIZE = 56;
const SOCKET_LEVEL = 1;
const RIGHTS = 1;
const CREDENTIALS = 2;
const DONTWAIT = 0x40;
const CMSG_CLOEXEC = 0x40000000;

// msghdr/iovec/cmsghdr and constants are tied to this exact Linux x86_64 ABI.
// Kept behind the explicit transport factory so ordinary runs never load FFI.
const libcPath = "/usr/lib/x86_64-linux-gnu/libc.so.6";
const libcDigest = "3a15d66867d83762c7f2f1e37359cb8f6c5743edb369c65285cb0b1c4f7498bf";
export async function openNextOuterReceiver() {
  assert.equal(process.platform, "linux");
  assert.equal(process.arch, "x64");
  assert.equal(process.version, "v26.13.1");
  const ffi = await import("node:ffi");
  // Load the same descriptor whose bytes were verified. Initial hashing belongs
  // in bounded pre-launch preparation, outside the decision path.
  const fd = openSync(libcPath, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  let handle;
  try {
    const before = fstatSync(fd, { bigint: true });
    assert(before.isFile() && before.size > 0n && before.size <= 4n * 1024n * 1024n);
    const digest = createHash("sha256");
    const segment = Buffer.alloc(64 * 1024);
    for (let pos = 0; pos < Number(before.size);) {
      const want = Math.min(segment.length, Number(before.size) - pos);
      assert.equal(readSync(fd, segment, 0, want, pos), want);
      digest.update(segment.subarray(0, want));
      pos += want;
    }
    const after = fstatSync(fd, { bigint: true });
    assert(
      after.ctimeNs === before.ctimeNs &&
        after.mtimeNs === before.mtimeNs &&
        after.size === before.size,
    );
    assert.equal(digest.digest("hex"), libcDigest);
    handle = ffi.dlopen("/proc/self/fd/" + fd, {
      recvmsg: { arguments: ["int32", "buffer", "int32"], return: "int64" },
    });
  } finally {
    closeSync(fd);
  }
  return {
    close() {
      handle.lib.close();
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
      assert(bytes > 0 && bytes <= body.length, "missing or invalid datagram");
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
