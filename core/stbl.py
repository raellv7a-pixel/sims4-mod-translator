import struct

class STBLResource:
    """Parser and Compiler for Sims 4 STBL (String Table v5) resources."""
    
    MAGIC = b'STBL'
    VERSION = 5
    
    def __init__(self, raw_bytes=None):
        self.entries = []
        if raw_bytes:
            self.parse(raw_bytes)
            
    def parse(self, raw_bytes):
        if len(raw_bytes) < 21 or raw_bytes[:4] != self.MAGIC:
            raise ValueError("Invalid STBL resource header.")
            
        version = struct.unpack("<H", raw_bytes[4:6])[0]
        if version != self.VERSION:
            raise ValueError(f"Unsupported STBL version: {version}.")

        count = struct.unpack("<Q", raw_bytes[7:15])[0]
        total_string_bytes = struct.unpack("<I", raw_bytes[17:21])[0]
        
        self.entries = []
        off = 21
        actual_string_bytes = 0
        for _ in range(count):
            if off + 7 > len(raw_bytes):
                raise ValueError("Truncated STBL entry.")
            key = struct.unpack("<I", raw_bytes[off:off+4])[0]
            flags = raw_bytes[off+4]
            str_len = struct.unpack("<H", raw_bytes[off+5:off+7])[0]
            if off + 7 + str_len > len(raw_bytes):
                raise ValueError("Truncated STBL string.")
            str_val = raw_bytes[off+7 : off+7+str_len].decode('utf-8')
            
            self.entries.append({
                "key": key,
                "key_hex": f"0x{key:08X}",
                "flags": flags,
                "value": str_val
            })
            off += 7 + str_len

            # STBL v5 counts the per-string flags byte in this header field.
            actual_string_bytes += 1 + str_len

        if actual_string_bytes != total_string_bytes:
            raise ValueError("STBL string byte count does not match its header.")
        if off != len(raw_bytes):
            raise ValueError("Unexpected trailing bytes in STBL resource.")
            
        return self.entries
        
    def compile(self):
        count = len(self.entries)
        entries_bytes = bytearray()
        total_string_bytes = 0
        
        for item in self.entries:
            key = item["key"] if isinstance(item["key"], int) else int(item["key"], 16)
            flags = item["flags"]
            val_utf8 = item["value"].encode('utf-8')
            val_len = len(val_utf8)
            if val_len > 0xFFFF:
                raise ValueError(f"STBL string 0x{key:08X} exceeds the 65535-byte limit.")
            # STBL v5 counts the per-string flags byte in this header field.
            total_string_bytes += 1 + val_len
            
            entries_bytes.extend(struct.pack("<I", key))
            entries_bytes.append(flags)
            entries_bytes.extend(struct.pack("<H", val_len))
            entries_bytes.extend(val_utf8)
            
        header = bytearray()
        header.extend(self.MAGIC)
        header.extend(struct.pack("<H", self.VERSION))
        header.append(0)
        header.extend(struct.pack("<Q", count))
        header.extend(b'\x00\x00') # 2 reserved bytes
        header.extend(struct.pack("<I", total_string_bytes))
        
        return bytes(header + entries_bytes)
