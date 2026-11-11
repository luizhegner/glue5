"""CI-only interoperability: synthetic libopus -> AWC -> API -> Ogg -> FFmpeg.

Never run on the owner's PC. Uses no game bytes, keys or downloaded fixtures.
"""
import ctypes as c
import json
import math
from pathlib import Path
import struct
import subprocess
import tempfile

opus = c.CDLL("libopus.so.0")
opus.opus_encoder_create.argtypes = [c.c_int, c.c_int, c.c_int, c.POINTER(c.c_int)]
opus.opus_encoder_create.restype = c.c_void_p
opus.opus_encode.argtypes = [c.c_void_p, c.POINTER(c.c_int16), c.c_int, c.POINTER(c.c_ubyte), c.c_int]
opus.opus_encode.restype = c.c_int
opus.opus_encoder_destroy.argtypes = [c.c_void_p]
opus.opus_decoder_create.argtypes = [c.c_int, c.c_int, c.POINTER(c.c_int)]
opus.opus_decoder_create.restype = c.c_void_p
opus.opus_decode.argtypes = [c.c_void_p, c.POINTER(c.c_ubyte), c.c_int, c.POINTER(c.c_int16), c.c_int, c.c_int]
opus.opus_decode.restype = c.c_int
opus.opus_decoder_destroy.argtypes = [c.c_void_p]
error = c.c_int()
encoder = opus.opus_encoder_create(48000, 1, 2049, c.byref(error))
assert encoder and error.value == 0
decoder = opus.opus_decoder_create(48000, 1, c.byref(error))
assert decoder and error.value == 0
packets, reference = [], []
try:
    # 16 kbps keeps each 60 ms packet within the original u8 framing.
    assert opus.opus_encoder_ctl(c.c_void_p(encoder), c.c_int(4002), c.c_int(16000)) == 0
    for frame in range(3):
        pcm = (c.c_int16 * 2880)(*(int(5000 * math.sin(2 * math.pi * 440 * (frame*2880+i)/48000)) for i in range(2880)))
        packet = (c.c_ubyte * 255)()
        size = opus.opus_encode(encoder, pcm, 2880, packet, 255)
        assert 0 < size <= 255
        packets.append(bytes(packet[:size]))
        output = (c.c_int16 * 2880)()
        assert opus.opus_decode(decoder, packet, size, output, 2880, 0) == 2880
        reference.extend(output)
finally:
    opus.opus_encoder_destroy(encoder)
    opus.opus_decoder_destroy(decoder)

payload = b"".join(bytes([len(packet)]) + packet for packet in packets)
samples = 8500  # Prove exact end trimming, not just full-packet counts.
awc = bytearray(68 + len(payload))
awc[:4] = b"ADAT"
struct.pack_into("<IIII", awc, 4, 0xff020001, 1, 48, (2 << 29) | 123)
struct.pack_into("<Q", awc, 20, (0xfa << 56) | (20 << 28) | 48)
struct.pack_into("<Q", awc, 28, (0x55 << 56) | (len(payload) << 28) | 68)
struct.pack_into("<I", awc, 48, samples)
struct.pack_into("<H", awc, 56, 48000)
awc[67] = 12
awc[68:] = payload
with tempfile.TemporaryDirectory(prefix="glue5-synthetic-opus-") as temporary:
    root = Path(temporary)
    source, cache = root / "source", root / "cache"
    source.mkdir()
    cache.mkdir()
    (source / "synthetic.awc").write_bytes(awc)
    request = {"jsonrpc": "2.0", "id": 1, "method": "audio.extract_web_opus",
               "params": {"uri": "synthetic.awc", "stream_id": 123, "build": "8b0b5899ed"}}
    child = subprocess.run(["target/debug/glue5-resource-api", "--source", str(source), "--cache", str(cache)],
                           input=json.dumps(request)+"\n", text=True, capture_output=True, check=True)
    response = json.loads(child.stdout)
    assert "error" not in response, response
    result = response["result"]
    ogg = cache / result["bundle"] / "audio.ogg"
    # Use the same reference decoder for PCM comparison. FFmpeg's separate
    # native Opus implementation is not sample-identical to libopus at SILK
    # bitrates; that difference is unrelated to packet-preserving repacking.
    decoded = subprocess.run(["ffmpeg", "-v", "error", "-c:a", "libopus", "-i", str(ogg), "-f", "s16le", "-acodec", "pcm_s16le", "-"],
                             capture_output=True, check=True).stdout
    assert len(decoded) == samples*2, len(decoded)
    actual = struct.unpack("<" + "h"*samples, decoded)
    rms_error = math.sqrt(sum((a-b)**2 for a, b in zip(actual, reference[:samples])) / samples)
    assert rms_error < 4, rms_error
    assert (source / "synthetic.awc").read_bytes() == awc
    assert result["receipt"]["output"] == "audio.ogg"
    assert result["receipt"]["selection"]["build"] == "8b0b5899ed"
    print(f"Synthetic Web Opus interoperability passed: {len(packets)} unchanged packets, {samples} mono samples, RMS error {rms_error:.3f}")
