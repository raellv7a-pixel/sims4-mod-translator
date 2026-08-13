import struct
import zlib

class DBPFPackage:
    """Parser for Sims 4 DBPF 2.1 database package files."""
    
    TYPE_STBL = 0x220557DA
    
    def __init__(self, file_path):
        self.file_path = file_path
        self.entries = []
        self.stbl_entries = []
        self.raw_data = b""
        self.load()
        
    def load(self):
        with open(self.file_path, "rb") as f:
            self.raw_data = f.read()
            
        if len(self.raw_data) < 96 or self.raw_data[:4] != b'DBPF':
            raise ValueError("Invalid DBPF package file (missing DBPF magic header).")
            
        header = self.raw_data[:96]
        count = struct.unpack("<I", header[36:40])[0]
        idx_size = struct.unpack("<I", header[44:48])[0]
        idx_off = struct.unpack("<I", header[64:68])[0]

        if idx_off < len(header) or idx_size > len(self.raw_data) - idx_off:
            raise ValueError("Invalid DBPF index range.")

        index_bytes = self.raw_data[idx_off:idx_off + idx_size]

        # Check if 4-byte header flag exists in index
        entry_offset_start = 4 if len(index_bytes) % 32 == 4 else 0

        if len(index_bytes) != entry_offset_start + count * 32:
            raise ValueError("Invalid DBPF index size or entry count.")
        
        self.entries = []
        self.stbl_entries = []
        
        for i in range(count):
            start = entry_offset_start + i * 32
            e = index_bytes[start : start + 32]
            if len(e) < 32:
                break
            t_id, g_id, inst_h, inst_l, res_off, c_size_flag, u_size, comp_type = struct.unpack("<IIIIIIII", e)
            c_size = c_size_flag & 0x7FFFFFFF
            is_comp = bool(c_size_flag & 0x80000000)
            lang_code = (inst_h >> 24) & 0xFF

            if res_off < len(header) or c_size > idx_off - res_off:
                raise ValueError(f"Invalid resource range at index {i}.")
            
            entry = {
                "index": i,
                "type": t_id,
                "group": g_id,
                "inst_h": inst_h,
                "inst_l": inst_l,
                "instance_hex": f"0x{inst_h:08X}{inst_l:08X}",
                "offset": res_off,
                "c_size_flag": c_size_flag,
                "c_size": c_size,
                "u_size": u_size,
                "comp_type": comp_type,
                "is_comp": is_comp,
                "lang_code": lang_code
            }
            
            self.entries.append(entry)
            if t_id == self.TYPE_STBL:
                self.stbl_entries.append(entry)
                
    def read_payload(self, entry):
        off = entry["offset"]
        c_size = entry["c_size"]
        u_size = entry["u_size"]
        raw_bytes = self.raw_data[off : off + c_size]
        
        if entry["is_comp"]:
            if entry["comp_type"] != 0x15A42:
                raise ValueError(f"Unsupported compression type: 0x{entry['comp_type']:X}")
            try:
                payload = zlib.decompress(raw_bytes)
            except zlib.error as exc:
                raise ValueError("Invalid compressed DBPF resource.") from exc
            if len(payload) != u_size:
                raise ValueError("Compressed DBPF resource size does not match its index.")
            return payload
        return raw_bytes
