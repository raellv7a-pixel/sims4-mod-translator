use anyhow::{bail, Context, Result};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

const DBPF_HEADER_SIZE: usize = 96;
const DBPF_ENTRY_SIZE: usize = 32;
const STBL_TYPE: u32 = 0x2205_57DA;
const ZLIB_COMPRESSION: u32 = 0x0001_5A42;
const LANGUAGE_CODES: [u8; 18] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
    0x11, 0x12, 0x13, 0x15,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceKey {
    pub group: u32,
    pub instance_high: u32,
    pub instance_low: u32,
}

impl ResourceKey {
    pub fn base_instance_high(&self) -> u32 {
        self.instance_high & 0x00FF_FFFF
    }
}

#[derive(Clone, Debug)]
struct DbpfEntry {
    type_id: u32,
    key: ResourceKey,
    offset: usize,
    compressed_size: usize,
    uncompressed_size: usize,
    compression_type: u32,
    compressed: bool,
}

#[derive(Clone, Debug)]
pub struct StringEntry {
    pub key: u32,
    pub flags: u8,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct StblTable {
    pub key: ResourceKey,
    pub entries: Vec<StringEntry>,
}

pub struct Package {
    bytes: Vec<u8>,
    entries: Vec<DbpfEntry>,
}

impl Package {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("Não foi possível ler {}", path.display()))?;
        if bytes.len() < DBPF_HEADER_SIZE || &bytes[0..4] != b"DBPF" {
            bail!("O arquivo não é um pacote DBPF válido.");
        }

        let count = read_u32(&bytes, 36)? as usize;
        let index_size = read_u32(&bytes, 44)? as usize;
        let index_offset = read_u32(&bytes, 64)? as usize;
        let index_end = index_offset
            .checked_add(index_size)
            .context("Índice DBPF inválido.")?;
        if index_offset < DBPF_HEADER_SIZE || index_end > bytes.len() {
            bail!("O índice DBPF está fora dos limites do arquivo.");
        }

        let index_header = match index_size % DBPF_ENTRY_SIZE {
            0 => 0,
            4 => 4,
            _ => bail!("O tamanho do índice DBPF é inválido."),
        };
        if index_size != index_header + count * DBPF_ENTRY_SIZE {
            bail!("A quantidade de recursos DBPF não corresponde ao índice.");
        }

        let mut entries = Vec::with_capacity(count);
        for resource_index in 0..count {
            let offset = index_offset + index_header + resource_index * DBPF_ENTRY_SIZE;
            let type_id = read_u32(&bytes, offset)?;
            let group = read_u32(&bytes, offset + 4)?;
            let instance_high = read_u32(&bytes, offset + 8)?;
            let instance_low = read_u32(&bytes, offset + 12)?;
            let payload_offset = read_u32(&bytes, offset + 16)? as usize;
            let size_and_flag = read_u32(&bytes, offset + 20)?;
            let compressed_size = (size_and_flag & 0x7FFF_FFFF) as usize;
            let payload_end = payload_offset
                .checked_add(compressed_size)
                .context("Recurso DBPF inválido.")?;
            if payload_offset < DBPF_HEADER_SIZE || payload_end > index_offset {
                bail!("O recurso DBPF {resource_index} está fora dos limites do pacote.");
            }

            entries.push(DbpfEntry {
                type_id,
                key: ResourceKey { group, instance_high, instance_low },
                offset: payload_offset,
                compressed_size,
                uncompressed_size: read_u32(&bytes, offset + 24)? as usize,
                compression_type: read_u32(&bytes, offset + 28)?,
                compressed: size_and_flag & 0x8000_0000 != 0,
            });
        }

        Ok(Self { bytes, entries })
    }

    pub fn all_string_tables(&self) -> Result<Vec<(ResourceKey, Vec<StringEntry>)>> {
        let stbl_entries: Vec<&DbpfEntry> = self
            .entries
            .iter()
            .filter(|entry| entry.type_id == STBL_TYPE)
            .collect();

        if stbl_entries.is_empty() {
            bail!("O pacote não contém nenhuma tabela de textos (STBL).");
        }

        use std::collections::BTreeMap;
        let mut unique_tables: BTreeMap<(u32, u32, u32), &DbpfEntry> = BTreeMap::new();

        for entry in stbl_entries {
            let base_h = entry.key.base_instance_high();
            let key_tuple = (entry.key.group, base_h, entry.key.instance_low);
            let lang_code = (entry.key.instance_high >> 24) & 0xFF;

            match unique_tables.get(&key_tuple) {
                None => {
                    unique_tables.insert(key_tuple, entry);
                }
                Some(existing) => {
                    let existing_lang = (existing.key.instance_high >> 24) & 0xFF;
                    if lang_code == 0x00 && existing_lang != 0x00 {
                        unique_tables.insert(key_tuple, entry);
                    }
                }
            }
        }

        let mut result = Vec::new();
        for (_, entry) in unique_tables {
            let payload = self.read_payload(entry)?;
            let parsed = parse_stbl(&payload)?;
            result.push((entry.key, parsed));
        }

        Ok(result)
    }

    #[allow(dead_code)]
    pub fn first_string_table(&self) -> Result<(ResourceKey, Vec<StringEntry>)> {
        let tables = self.all_string_tables()?;
        tables.into_iter().next().context("O pacote não contém uma tabela de textos (STBL).")
    }

    fn read_payload(&self, entry: &DbpfEntry) -> Result<Vec<u8>> {
        let bytes = &self.bytes[entry.offset..entry.offset + entry.compressed_size];
        if !entry.compressed {
            return Ok(bytes.to_vec());
        }
        if entry.compression_type != ZLIB_COMPRESSION {
            bail!("O pacote usa uma compressão DBPF não suportada.");
        }
        let mut decoder = ZlibDecoder::new(bytes);
        let mut payload = Vec::with_capacity(entry.uncompressed_size);
        decoder.read_to_end(&mut payload).context("Recurso DBPF comprimido inválido.")?;
        if payload.len() != entry.uncompressed_size {
            bail!("O tamanho descomprimido do recurso DBPF não corresponde ao índice.");
        }
        Ok(payload)
    }
}

pub fn create_translation_package(tables: &[StblTable]) -> Result<Vec<u8>> {
    if tables.is_empty() {
        bail!("Nenhuma tabela STBL fornecida para exportação.");
    }

    struct PreparedTable {
        source_key: ResourceKey,
        compressed: Vec<u8>,
        uncompressed_len: usize,
    }

    let mut prepared = Vec::with_capacity(tables.len());
    let mut total_payload_bytes = 0_usize;

    for table in tables {
        for entry in &table.entries {
            if entry.value.contains('\0') {
                bail!("A chave 0x{:08X} contém um caractere NUL.", entry.key);
            }
            if !preserves_tokens(&entry.value, &entry.value) {
                bail!("A chave 0x{:08X} contém marcadores inválidos.", entry.key);
            }
        }

        let stbl = compile_stbl(&table.entries)?;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&stbl)?;
        let compressed = encoder.finish()?;
        total_payload_bytes += compressed.len();
        prepared.push(PreparedTable {
            source_key: table.key,
            compressed,
            uncompressed_len: stbl.len(),
        });
    }

    let total_index_entries = prepared.len() * LANGUAGE_CODES.len();
    let index_size = 4 + total_index_entries * DBPF_ENTRY_SIZE;
    let index_offset = DBPF_HEADER_SIZE + total_payload_bytes;

    let mut package = vec![0_u8; DBPF_HEADER_SIZE];
    package[0..4].copy_from_slice(b"DBPF");
    write_u32(&mut package, 4, 2)?;
    write_u32(&mut package, 8, 1)?;
    write_u32(&mut package, 36, total_index_entries as u32)?;
    write_u32(&mut package, 44, index_size as u32)?;
    write_u32(&mut package, 60, 3)?;
    write_u32(&mut package, 64, index_offset as u32)?;

    let mut offsets = Vec::with_capacity(prepared.len());
    let mut current_offset = DBPF_HEADER_SIZE;

    for item in &prepared {
        offsets.push(current_offset);
        package.extend_from_slice(&item.compressed);
        current_offset += item.compressed.len();
    }

    package.extend_from_slice(&[0, 0, 0, 0]);

    for (item, &offset) in prepared.iter().zip(offsets.iter()) {
        for language in LANGUAGE_CODES {
            let instance_high = ((language as u32) << 24) | item.source_key.base_instance_high();
            for value in [
                STBL_TYPE,
                item.source_key.group,
                instance_high,
                item.source_key.instance_low,
                offset as u32,
                item.compressed.len() as u32 | 0x8000_0000,
                item.uncompressed_len as u32,
                ZLIB_COMPRESSION,
            ] {
                package.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(package)
}

pub fn preserves_tokens(source: &str, translated: &str) -> bool {
    protected_tokens(source) == protected_tokens(translated)
}

fn parse_stbl(bytes: &[u8]) -> Result<Vec<StringEntry>> {
    if bytes.len() < 21 || &bytes[0..4] != b"STBL" {
        bail!("Cabeçalho STBL inválido.");
    }
    if read_u16(bytes, 4)? != 5 {
        bail!("A versão STBL não é suportada.");
    }
    let count = read_u64(bytes, 7)? as usize;
    let declared_string_bytes = read_u32(bytes, 17)? as usize;
    let mut offset = 21;
    let mut total_string_bytes = 0_usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        if offset + 7 > bytes.len() {
            bail!("Entrada STBL truncada.");
        }
        let key = read_u32(bytes, offset)?;
        let flags = bytes[offset + 4];
        let string_len = read_u16(bytes, offset + 5)? as usize;
        let end = offset + 7 + string_len;
        if end > bytes.len() {
            bail!("Texto STBL truncado.");
        }
        let value = std::str::from_utf8(&bytes[offset + 7..end])
            .context("Texto STBL não está em UTF-8.")?
            .to_owned();
        entries.push(StringEntry { key, flags, value });
        total_string_bytes += 1 + string_len;
        offset = end;
    }
    if offset != bytes.len() || total_string_bytes != declared_string_bytes {
        bail!("Os tamanhos declarados no STBL não correspondem ao conteúdo.");
    }
    Ok(entries)
}

fn compile_stbl(entries: &[StringEntry]) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut total_string_bytes = 0_u32;
    for entry in entries {
        let text = entry.value.as_bytes();
        if text.len() > u16::MAX as usize {
            bail!("A chave 0x{:08X} excede o limite de 65535 bytes.", entry.key);
        }
        total_string_bytes += 1 + text.len() as u32;
        data.extend_from_slice(&entry.key.to_le_bytes());
        data.push(entry.flags);
        data.extend_from_slice(&(text.len() as u16).to_le_bytes());
        data.extend_from_slice(text);
    }
    let mut output = Vec::with_capacity(21 + data.len());
    output.extend_from_slice(b"STBL");
    output.extend_from_slice(&5_u16.to_le_bytes());
    output.push(0);
    output.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    output.extend_from_slice(&[0, 0]);
    output.extend_from_slice(&total_string_bytes.to_le_bytes());
    output.extend_from_slice(&data);
    Ok(output)
}

fn protected_tokens(value: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let closing = match bytes[index] {
            b'{' => b'}',
            b'<' if bytes.get(index + 1).is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'/') => b'>',
            _ => {
                index += 1;
                continue;
            }
        };
        if let Some(end) = bytes[index + 1..].iter().position(|byte| *byte == closing) {
            let end = index + end + 1;
            tokens.push(value[index..=end].to_owned());
            index = end + 1;
        } else {
            index += 1;
        }
    }
    tokens
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let chunk: [u8; 2] = bytes.get(offset..offset + 2).context("Leitura fora dos limites.")?.try_into()?;
    Ok(u16::from_le_bytes(chunk))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let chunk: [u8; 4] = bytes.get(offset..offset + 4).context("Leitura fora dos limites.")?.try_into()?;
    Ok(u32::from_le_bytes(chunk))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let chunk: [u8; 8] = bytes.get(offset..offset + 8).context("Leitura fora dos limites.")?.try_into()?;
    Ok(u64::from_le_bytes(chunk))
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) -> Result<()> {
    let destination = bytes.get_mut(offset..offset + 4).context("Escrita fora dos limites.")?;
    destination.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<StringEntry> {
        vec![
            StringEntry { key: 0xA97864D3, flags: 0, value: "<font color='#6b0ac5'><b>{0.String}</b></font> Intimidade".into() },
            StringEntry { key: 0xD4CC81A6, flags: 0, value: "Documento de relacionamento\n{1.String} {0.String}".into() },
        ]
    }

    #[test]
    fn stbl_round_trip() {
        let entries = sample_entries();
        let compiled = compile_stbl(&entries).unwrap();
        let parsed = parse_stbl(&compiled).unwrap();
        assert_eq!(parsed.len(), entries.len());
        for (a, b) in parsed.iter().zip(entries.iter()) {
            assert_eq!(a.key, b.key);
            assert_eq!(a.flags, b.flags);
            assert_eq!(a.value, b.value);
        }
    }

    #[test]
    fn package_round_trip() {
        let entries = sample_entries();
        let source = ResourceKey { group: 0, instance_high: 0x0007_401A, instance_low: 0x6778_F312 };
        let tables = vec![StblTable { key: source, entries }];
        let bytes = create_translation_package(&tables).unwrap();
        let path = std::env::temp_dir().join("sims4_roundtrip_test.package");
        std::fs::write(&path, &bytes).unwrap();
        let package = Package::open(&path).unwrap();
        let all_tables = package.all_string_tables().unwrap();
        assert_eq!(all_tables.len(), 1);
        let (key, parsed) = &all_tables[0];
        assert_eq!(key.group, source.group);
        assert_eq!(key.instance_high & 0x00FF_FFFF, source.instance_high & 0x00FF_FFFF);
        assert_eq!(key.instance_low, source.instance_low);
        assert_eq!(parsed.len(), 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn token_preservation() {
        let text = "<font color='#6b0ac5'><b>{0.String}</b></font> {1.String}";
        assert!(preserves_tokens(text, "<font color='#6b0ac5'><b>{0.String}</b></font> texto {1.String}"));
        assert!(!preserves_tokens(text, "<b>{0.String}</b> texto {1.String}"));
        assert!(!preserves_tokens(text, "Texto sem marcadores"));
    }

    #[test]
    fn nul_rejected() {
        let mut entries = sample_entries();
        entries[0].value = "olá\0mundo".into();
        let tables = vec![StblTable { key: ResourceKey { group: 0, instance_high: 0x1, instance_low: 0x2 }, entries }];
        assert!(create_translation_package(&tables).is_err());
    }

    #[test]
    #[ignore = "requires the user's mod file on disk"]
    fn real_mod_round_trip() {
        let mod_path = std::path::Path::new("/home/raell/Downloads/OfficeAffairs/TURBODRIVER_WickedWhims_Tuning.package");
        if !mod_path.exists() {
            return;
        }
        let package = Package::open(mod_path).expect("open mod");
        let tables = package.all_string_tables().expect("read STBLs");
        assert_eq!(tables.len(), 5);
        let stbl_tables: Vec<StblTable> = tables.into_iter().map(|(key, entries)| StblTable { key, entries }).collect();
        let bytes = create_translation_package(&stbl_tables).expect("generate");
        let out = std::env::temp_dir().join("sims4_real_mod_test.package");
        std::fs::write(&out, &bytes).unwrap();
        let reopened = Package::open(&out).expect("reopen");
        let parsed_tables = reopened.all_string_tables().expect("reparse");
        assert_eq!(parsed_tables.len(), 5);
        std::fs::remove_file(&out).ok();
    }
}
