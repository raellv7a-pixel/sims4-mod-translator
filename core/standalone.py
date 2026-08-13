import struct
import zlib
import os

class StandalonePackageGenerator:
    """Generates standalone Sims 4 translation .package files."""
    
    LANG_CODES = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x11, 0x12, 0x13, 0x15]
    
    @classmethod
    def generate(cls, stbl_uncompressed_bytes, base_high=0x0007401A, base_low=0x6778F312, group_id=0x00000000, output_path=None):
        stbl_compressed = zlib.compress(stbl_uncompressed_bytes, level=9)
        
        payload_offset = 96
        body = bytearray(stbl_compressed)
        
        index_entries = bytearray()
        index_header_flag = b'\x00\x00\x00\x00'
        
        for lang in cls.LANG_CODES:
            inst_h = (lang << 24) | (base_high & 0x00FFFFFF)
            inst_l = base_low
            
            entry = struct.pack("<IIIIIIII",
                0x220557DA, # STBL Type ID
                group_id,   # Group ID
                inst_h,     # Instance High
                inst_l,     # Instance Low
                payload_offset,
                len(stbl_compressed) | 0x80000000,
                len(stbl_uncompressed_bytes),
                0x00015A42  # ZLIB compressed
            )
            index_entries.extend(entry)
            
        idx_offset = 96 + len(body)
        idx_size = len(index_header_flag) + len(index_entries)
        
        header = bytearray(96)
        header[0:4] = b'DBPF'
        struct.pack_into("<I", header, 4, 2)
        struct.pack_into("<I", header, 8, 1)
        struct.pack_into("<I", header, 36, len(cls.LANG_CODES))
        struct.pack_into("<I", header, 44, idx_size)
        struct.pack_into("<I", header, 60, 3)
        struct.pack_into("<I", header, 64, idx_offset)
        
        package_bytes = bytes(header + body + index_header_flag + index_entries)
        
        if output_path:
            with open(output_path, "wb") as f:
                f.write(package_bytes)
                
        return package_bytes
