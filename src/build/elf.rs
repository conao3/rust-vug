use std::ops::Range;

use object::elf;
use object::read::elf::{Dyn, FileHeader, ProgramHeader};
use object::{Endianness, StringTable};

#[derive(Default)]
pub(super) struct Dynamic {
    pub(super) needed: Vec<Range<usize>>,
    pub(super) search_paths: Vec<Range<usize>>,
    pub(super) runpath: Option<Range<usize>>,
    pub(super) interpreter: Option<Range<usize>>,
}

pub(super) fn dynamic(data: &[u8]) -> Result<Option<Dynamic>, String> {
    if !data.starts_with(&elf::ELFMAG) {
        return Ok(None);
    }
    match data.get(std::mem::offset_of!(elf::Ident, class)).copied() {
        Some(elf::ELFCLASS32) => parse::<elf::FileHeader32<Endianness>>(data),
        Some(elf::ELFCLASS64) => parse::<elf::FileHeader64<Endianness>>(data),
        _ => Err("invalid ELF class".into()),
    }
    .map(Some)
}

pub(super) fn loadable(data: &[u8]) -> bool {
    fn kind<Elf: FileHeader>(data: &[u8]) -> Option<u16> {
        let header = Elf::parse(data).ok()?;
        let endian = header.endian().ok()?;
        Some(header.e_type(endian))
    }
    let object_type = match data.get(std::mem::offset_of!(elf::Ident, class)).copied() {
        Some(elf::ELFCLASS32) => kind::<elf::FileHeader32<Endianness>>(data),
        Some(elf::ELFCLASS64) => kind::<elf::FileHeader64<Endianness>>(data),
        _ => None,
    };
    matches!(object_type, Some(elf::ET_EXEC | elf::ET_DYN))
}

fn parse<Elf: FileHeader>(data: &[u8]) -> Result<Dynamic, String> {
    let error = |error: object::Error| error.to_string();
    let elf = Elf::parse(data).map_err(error)?;
    let endian = elf.endian().map_err(error)?;
    let headers = elf.program_headers(endian, data).map_err(error)?;
    let mut result = Dynamic::default();
    let mut entries = &[][..];
    let range = |bytes: &[u8]| {
        let start = bytes.as_ptr() as usize - data.as_ptr() as usize;
        start..start + bytes.len()
    };
    for header in headers {
        if let Some(path) = header.interpreter(endian, data).map_err(error)? {
            result.interpreter = Some(range(path));
        }
        if let Some(dynamic) = header.dynamic(endian, data).map_err(error)? {
            entries = dynamic;
        }
    }
    let entries = entries
        .split(|entry| entry.tag(endian) == elf::DT_NULL)
        .next()
        .unwrap();
    let value = |tag| {
        entries
            .iter()
            .find(|entry| entry.tag(endian) == tag)
            .map(|entry| entry.val(endian))
    };
    let has_paths = entries.iter().any(|entry| {
        matches!(
            entry.tag(endian),
            elf::DT_RPATH | elf::DT_RUNPATH | elf::DT_NEEDED
        )
    });
    if !has_paths {
        return Ok(result);
    }

    let address = value(elf::DT_STRTAB).ok_or("ELF has no dynamic string table")?;
    let length = value(elf::DT_STRSZ).ok_or("ELF has no dynamic string table size")?;
    let strings = headers
        .iter()
        .filter(|header| header.p_type(endian) == elf::PT_LOAD)
        .find_map(|header| {
            header
                .data_range(endian, data, address, length)
                .ok()
                .flatten()
        })
        .ok_or("ELF dynamic string table is outside the load segments")?;
    let strings = StringTable::new(strings, 0, length);
    for entry in entries {
        let tag = entry.tag(endian);
        if matches!(tag, elf::DT_NEEDED | elf::DT_RPATH | elf::DT_RUNPATH) {
            let field = range(entry.string(endian, strings).map_err(error)?);
            if tag == elf::DT_NEEDED {
                result.needed.push(field);
            } else {
                if tag == elf::DT_RUNPATH {
                    result.runpath = Some(field.clone());
                }
                result.search_paths.push(field);
            }
        }
    }
    result
        .search_paths
        .sort_unstable_by_key(|range| range.start);
    result.search_paths.dedup();
    Ok(result)
}
