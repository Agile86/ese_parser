//reader.rs

#![allow(unused_assignments, temporary_cstring_as_ptr)]

use std::{fs, io, io::{Seek, Read}, mem, os::raw, path::PathBuf, ptr, slice, convert::TryInto, cell::RefCell};
use simple_error::SimpleError;
use cache_2q::Cache;

use crate::parser::ese_db;
use crate::parser::ese_db::*;
use crate::parser::jet;

extern "C" {
    fn decompress(  data: *const u8,
                    data_size: u32,
                    out_buffer: *mut u8,
                    out_buffer_size: u32,
                    decompressed: *mut u32) -> u32;
}

pub struct Reader {
    file: RefCell<fs::File>,
    cache: RefCell<Cache<usize, Vec<u8>>>,
    format_version: jet::FormatVersion,
    format_revision: jet::FormatRevision,
    page_size: u64,
    last_page_number: u32,
}

#[allow(clippy::mut_from_ref)]
unsafe fn _any_as_slice<'a, U: Sized, T: Sized>(p: &'a &mut T) -> &'a mut [U] {
    slice::from_raw_parts_mut(
        (*p as *const T) as *mut U,
        mem::size_of::<T>() / mem::size_of::<U>(),
    )
}

impl Reader {

    fn load_db_file_header(&mut self) -> Result<ese_db::FileHeader, SimpleError> {

        let mut db_file_header =
            self.read_struct::<ese_db::FileHeader>(0)?;

        if db_file_header.signature != ESEDB_FILE_SIGNATURE {
            return Err(SimpleError::new("bad file_header.signature"));
        }

        fn calc_crc32(file_header: &&mut ese_db::FileHeader) -> u32 {
            let vec32: &[u32] = unsafe { _any_as_slice::<u32, _>(&file_header) };
            vec32.iter().skip(1).fold(0x89abcdef, |crc, &val| crc ^ val)
        }

        let stored_checksum = db_file_header.checksum;
        let checksum = calc_crc32(&&mut db_file_header);
        if stored_checksum != checksum {
            return Err(SimpleError::new(format!("wrong checksum: {}, calculated {}", stored_checksum, checksum)));
        }

        let backup_file_header = self.read_struct::<ese_db::FileHeader>(db_file_header.page_size as u64)?;

        if db_file_header.format_revision == 0 {
            db_file_header.format_revision = backup_file_header.format_revision;
        }

        if db_file_header.format_revision != backup_file_header.format_revision {
            return Err(SimpleError::new(format!(
                "mismatch in format revision: {} not equal to backup value {}",
                db_file_header.format_revision, backup_file_header.format_revision)));
        }

        if db_file_header.page_size == 0 {
            db_file_header.page_size = backup_file_header.page_size;
        }

        if db_file_header.page_size != backup_file_header.page_size {
            return Err(SimpleError::new(format!(
                "mismatch in page size: {} not equal to backup value {}",
                db_file_header.page_size, backup_file_header.page_size)));
        }
        if db_file_header.format_version != 0x620 {
            return Err(SimpleError::new(format!("unsupported format version: {}", db_file_header.format_version)));
        }

        Ok(db_file_header)
    }

    fn new(path: &PathBuf, cache_size: usize) -> Result<Reader, SimpleError> {
        let f = match fs::File::open(path) {
            Ok(f) => f,
            Err(e) => return Err(SimpleError::new(format!("File::open failed: {:?}", e)))
        };
        let mut reader = Reader {
            file: RefCell::new(f),
            cache: RefCell::new(Cache::new(cache_size)),
            page_size: 2 * 1024, //just to read header
            format_version: 0,
            format_revision: 0,
            last_page_number: 0
        };

        let db_fh = reader.load_db_file_header()?;
        reader.format_version = db_fh.format_version;
        reader.format_revision = db_fh.format_revision;
        reader.page_size = db_fh.page_size as u64;
        reader.last_page_number = (db_fh.page_size * 2) / db_fh.page_size;

        reader.cache.get_mut().clear();

        Ok(reader)
    }

    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<(), SimpleError> {
        let pg_no = (offset / self.page_size as u64) as usize;
        let mut c = self.cache.borrow_mut();
        if !c.contains_key(&pg_no) {
            let mut page_buf = vec![0u8; self.page_size as usize];
            let mut f = self.file.borrow_mut();
            match f.seek(io::SeekFrom::Start(pg_no as u64 * self.page_size as u64)) {
                Ok(_) => {
                    match f.read_exact(&mut page_buf) {
                        Ok(_) => {
                            c.insert(pg_no, page_buf);
                        },
                        Err(e) => {
                            return Err(SimpleError::new(format!("read_exact failed: {:?}", e)));
                        }
                    }
                },
                Err(e) => {
                    return Err(SimpleError::new(format!("seek failed: {:?}", e)));
                }
            }
        }

        match c.get(&pg_no) {
            Some(page_buf) => {
                let page_offset = (offset % self.page_size as u64) as usize;
                buf.copy_from_slice(&page_buf[page_offset..page_offset + buf.len()]);
            },
            None => {
                return Err(SimpleError::new(format!("Cache failed, page number not found: {}", pg_no)));
            }
        }

        Ok(())
    }

    pub fn read_struct<T>(&self, offset: u64) -> Result<T, SimpleError> {
        let struct_size = mem::size_of::<T>();
        let mut rec: T = unsafe { mem::zeroed() };
        unsafe {
            let buffer = slice::from_raw_parts_mut(&mut rec as *mut _ as *mut u8, struct_size);
            self.read(offset, buffer)?;
        }
        Ok(rec)
    }

    pub fn read_bytes(&self, offset: u64, size: usize) -> Result<Vec<u8>, SimpleError> {
        let mut buf = vec!(0u8; size);
        self.read(offset, &mut buf)?;
        Ok(buf)
    }

    pub fn read_string(&self, offset: u64, size: usize) -> Result<String, SimpleError> {
        let v = self.read_bytes(offset, size)?;
        match std::str::from_utf8(&v) {
            Ok(s) => Ok(s.to_string()),
            Err(e) => Err(SimpleError::new(format!("from_utf8 failed: error_len() is {:?}", e.error_len())))
        }
    }

    pub fn load_db(path: &std::path::PathBuf, cache_size: usize) -> Result<Reader, SimpleError> {
        Reader::new(path, cache_size)
    }
}

pub fn load_page_header(
    reader: &Reader,
    page_number: u32,
) -> Result<PageHeader, SimpleError> {
    let page_offset = (page_number + 1) as u64 * (reader.page_size) as u64;

    if reader.format_revision < ESEDB_FORMAT_REVISION_NEW_RECORD_FORMAT {
        let header = reader.read_struct::<PageHeaderOld>(page_offset)?;
        let common = reader.read_struct::<PageHeaderCommon>(page_offset + mem::size_of_val(&header) as u64)?;

        //let TODO_checksum = 0;
        Ok(PageHeader::old(header, common))
    } else if reader.format_revision < ESEDB_FORMAT_REVISION_EXTENDED_PAGE_HEADER {
        let header = reader.read_struct::<PageHeader0x0b>(page_offset)?;
        let common = reader.read_struct::<PageHeaderCommon>(page_offset + mem::size_of_val(&header) as u64)?;

        //TODO: verify checksum
        Ok(PageHeader::x0b(header, common))
    } else {
        let header = reader.read_struct::<PageHeader0x11>(page_offset)?;
        let common = reader.read_struct::<PageHeaderCommon>(page_offset + mem::size_of_val(&header) as u64)?;

        //TODO: verify checksum
        if reader.page_size > 8 * 1024 {
            let offs = mem::size_of_val(&header) + mem::size_of_val(&common);
            let ext = reader.read_struct::<PageHeaderExt0x11>(page_offset + offs as u64)?;

            Ok(PageHeader::x11_ext(header, common, ext))
        } else {
            Ok(PageHeader::x11(header, common))
        }
    }
}

pub fn load_page_tags(
    reader: &Reader,
    db_page: &jet::DbPage,
) -> Result<Vec<PageTag>, SimpleError> {
    let page_offset = (db_page.page_number + 1) as u64 * reader.page_size;
    let tags_cnt = db_page.get_available_page_tag();
    let mut tags_offset = (page_offset + reader.page_size as u64) as u64;
    let mut tags = Vec::<PageTag>::with_capacity(tags_cnt);

    for _i in 0..tags_cnt {
        tags_offset -= 2;
        let page_tag_offset : u16 = reader.read_struct(tags_offset)?;
        tags_offset -= 2;
        let page_tag_size : u16 = reader.read_struct(tags_offset)?;

        let flags : u8;
		let offset : u16;
        let size : u16;

        if reader.format_revision >= ESEDB_FORMAT_REVISION_EXTENDED_PAGE_HEADER && reader.page_size >= 16384 {
			offset = page_tag_offset & 0x7fff;
            size   = page_tag_size & 0x7fff;

            // The upper 3-bits of the first 16-bit-value in the leaf page entry contain the page tag flags
            //if db_page.flags().contains(jet::PageFlags::IS_LEAF)
            {
                let flags_offset = page_offset + db_page.size() as u64 + offset as u64;
                let f : u16 = reader.read_struct(flags_offset)?;
                flags = (f >> 13) as u8;
            }
        } else {
            flags  = (page_tag_offset >> 13) as u8;
            offset = page_tag_offset & 0x1fff;
            size   = page_tag_size & 0x1fff;
        }
        tags.push(PageTag{ flags, offset, size } );
    }

    Ok(tags)
}

pub fn load_root_page_header(
    reader: &Reader,
    db_page: &jet::DbPage,
    page_tag: &PageTag,
) -> Result<RootPageHeader, SimpleError> {
    let page_offset = (db_page.page_number + 1) as u64 * reader.page_size;
    let root_page_offset = page_offset + db_page.size() as u64 + page_tag.offset as u64;

    // TODO Seen in format version 0x620 revision 0x14
    // check format and revision
    if page_tag.size == 16 {
        let root_page_header = reader.read_struct::<ese_db::RootPageHeader16>(root_page_offset)?;
        return Ok(RootPageHeader::xf(root_page_header));
    } else if page_tag.size == 25 {
        let root_page_header = reader.read_struct::<ese_db::RootPageHeader25>(root_page_offset)?;
        return Ok(RootPageHeader::x19(root_page_header));
    }

    Err(SimpleError::new(format!("wrong size of page tag: {:?}", page_tag)))
}

pub fn page_tag_get_branch_child_page_number(
    reader: &Reader,
    db_page: &jet::DbPage,
    page_tag: &PageTag,
) -> Result<u32, SimpleError> {
    let page_offset = (db_page.page_number + 1) as u64 * reader.page_size as u64;
    let mut offset = page_offset + db_page.size() as u64 + page_tag.offset as u64;

    if page_tag.flags().intersects(jet::PageTagFlags::FLAG_HAS_COMMON_KEY_SIZE) {
        //let common_page_key_size : u16 = reader.read_struct(offset)?;
        offset += 2;
    }
    let local_page_key_size : u16 = reader.read_struct(offset)?;
    offset += 2;
    offset += local_page_key_size as u64;

    let child_page_number : u32 = reader.read_struct(offset)?;

    Ok(child_page_number)
}

pub fn load_catalog(
    reader: &Reader,
) -> Result<Vec<jet::TableDefinition>, SimpleError> {
    let db_page = jet::DbPage::new(reader, jet::FixedPageNumber::Catalog as u32)?;
    let pg_tags = &db_page.page_tags;

    let is_root = db_page.flags().contains(jet::PageFlags::IS_ROOT);

    if is_root {
        let _root_page_header = load_root_page_header(reader, &db_page, &pg_tags[0])?;
        //println!("root_page {:?}", root_page_header);
    }

    let mut res : Vec<jet::TableDefinition> = vec![];
    let mut table_def : jet::TableDefinition = jet::TableDefinition { table_catalog_definition: None,
        column_catalog_definition_array: vec![], long_value_catalog_definition: None };

    let mut page_number =
        if db_page.flags().contains(jet::PageFlags::IS_PARENT) {
            page_tag_get_branch_child_page_number(reader, &db_page, &pg_tags[1])?
        } else {
            if db_page.flags().contains(jet::PageFlags::IS_LEAF) {
                db_page.page_number
            } else {
                return Err(SimpleError::new(format!("pageno {}: IS_PARENT (branch) flag should be present in {:?}",
                                                    db_page.page_number, db_page.flags())));
            }
        };
    let mut prev_page_number = db_page.page_number;

    while page_number != 0 {
        let db_page = jet::DbPage::new(reader, page_number)?;
        let pg_tags = &db_page.page_tags;

        if db_page.prev_page() != 0 && prev_page_number != db_page.prev_page() {
            return Err(SimpleError::new(format!("pageno {}: wrong previous_page number {}, expected {}",
                db_page.page_number, db_page.prev_page(), prev_page_number)));
        }
        if !db_page.flags().contains(jet::PageFlags::IS_LEAF) {
            return Err(SimpleError::new(format!("pageno {}: IS_LEAF flag should be present",
                db_page.page_number)));
        }

        for i in 1..pg_tags.len() {
            if jet::PageTagFlags::from_bits_truncate(pg_tags[i].flags).intersects(jet::PageTagFlags::FLAG_IS_DEFUNCT) {
                continue;
            }
            let cat_item = load_catalog_item(reader, &db_page, &pg_tags[i])?;
            if cat_item.cat_type == jet::CatalogType::Table as u16 {
                if table_def.table_catalog_definition.is_some() {
                    res.push(table_def);
                    table_def = jet::TableDefinition { table_catalog_definition: None,
                        column_catalog_definition_array: vec![], long_value_catalog_definition: None };
                }
                table_def.table_catalog_definition = Some(cat_item);
            } else if cat_item.cat_type == jet::CatalogType::Column as u16 {
                table_def.column_catalog_definition_array.push(cat_item);
            } else if cat_item.cat_type == jet::CatalogType::Index as u16 {
                // TODO
            } else if cat_item.cat_type == jet::CatalogType::LongValue as u16 {
                if table_def.long_value_catalog_definition.is_some() {
                    return Err(SimpleError::new("long-value catalog definition duplicate?"));
                }
                table_def.long_value_catalog_definition = Some(cat_item);
            } else if cat_item.cat_type == jet::CatalogType::Callback as u16 {
                // TODO
            } else {
                println!("TODO: Unknown cat_item.cat_type {}", cat_item.cat_type);
            }
        }
        prev_page_number = page_number;
        page_number = db_page.next_page();
    }

    if table_def.table_catalog_definition.is_some() {
        res.push(table_def);
    }

    Ok(res)
}

pub fn load_catalog_item(
    reader: &Reader,
    db_page: &jet::DbPage,
    page_tag: &PageTag,
) -> Result<jet::CatalogDefinition, SimpleError> {
    let page_offset = (db_page.page_number + 1) as u64 * reader.page_size;
    let mut offset = page_offset + db_page.size() as u64 + page_tag.offset as u64;

    let mut first_word_readed = false;
    if page_tag.flags().intersects(jet::PageTagFlags::FLAG_HAS_COMMON_KEY_SIZE) {
        //let common_page_key_size : u16 = clean_pgtag_flag(reader, db_page, reader.read_struct::<u16>(offset)?);
        first_word_readed = true;
        offset += 2;
    }
    let mut local_page_key_size : u16 = reader.read_struct(offset)?;
    if !first_word_readed {
        local_page_key_size = clean_pgtag_flag(reader, db_page, local_page_key_size);
        first_word_readed = true;
    }
    offset += 2;
    offset += local_page_key_size as u64;

    let offset_ddh = offset;
    let ddh = reader.read_struct::<ese_db::DataDefinitionHeader>(offset_ddh)?;
    offset += mem::size_of::<ese_db::DataDefinitionHeader>() as u64;

    let mut number_of_variable_size_data_types : u32 = 0;
    if ddh.last_variable_size_data_type > 127 {
        number_of_variable_size_data_types = ddh.last_variable_size_data_type as u32 - 127;
    }

    let cat_def_zero = std::mem::MaybeUninit::<jet::CatalogDefinition>::zeroed();
    let mut cat_def = unsafe { cat_def_zero.assume_init() };
    let data_def = reader.read_struct::<ese_db::DataDefinition>(offset)?;

    cat_def.father_data_page_object_identifier = data_def.father_data_page_object_identifier;
    cat_def.cat_type = data_def.data_type;
    cat_def.identifier = data_def.identifier;
    if cat_def.cat_type == jet::CatalogType::Column as u16 {
        cat_def.column_type = unsafe { data_def.coltyp_or_fdp.column_type };
    } else {
        cat_def.father_data_page_number = unsafe { data_def.coltyp_or_fdp.father_data_page_number };
    }
    cat_def.size = data_def.space_usage;
    // data_def.flags?
    if cat_def.cat_type == jet::CatalogType::Column as u16 {
        cat_def.codepage = unsafe { data_def.pages_or_locale.codepage };
    }
    if ddh.last_fixed_size_data_type >= 10 {
        cat_def.lcmap_flags = data_def.lc_map_flags;
    }

    if number_of_variable_size_data_types > 0 {
        let mut variable_size_data_types_offset = ddh.variable_size_data_types_offset as u32;
        let variable_size_data_type_value_data_offset = variable_size_data_types_offset + (number_of_variable_size_data_types * 2);
        let mut previous_variable_size_data_type_size : u16 = 0;
        let mut data_type_number : u16 = 128;
        for _ in 0..number_of_variable_size_data_types {
            offset += ddh.variable_size_data_types_offset as u64;
            let variable_size_data_type_size : u16 = reader.read_struct(offset_ddh + variable_size_data_types_offset as u64)?;
            variable_size_data_types_offset += 2;

            let mut data_type_size : u16 = 0;
            if variable_size_data_type_size & 0x8000 != 0 {
                data_type_size = 0;
            } else {
                data_type_size = variable_size_data_type_size - previous_variable_size_data_type_size;
            }
            if data_type_size > 0 {
                match data_type_number {
                    128 => {
                        let offset_dtn = offset_ddh + variable_size_data_type_value_data_offset as u64 + previous_variable_size_data_type_size as u64;
                        cat_def.name = reader.read_string(offset_dtn, data_type_size as usize)?;
                        //println!("cat_def.name: {}", cat_def.name);
                    },
                    130 => {
                        // TODO template_name
                    },
                    131 => {
                        // TODO default_value
                        let offset_def = offset_ddh + variable_size_data_type_value_data_offset as u64 + previous_variable_size_data_type_size as u64;
                        cat_def.default_value = reader.read_bytes(offset_def, data_type_size as usize)?;
                    },
                    132 | // KeyFldIDs
                    133 | // VarSegMac
                    134 | // ConditionalColumns
                    135 | // TupleLimits
                    136   // Version
                        => {
                        // not usefull fields
                    },
                    _ => {
                        if data_type_size > 0 {
                            println!("TODO handle data_type_number {}", data_type_number);
                        }
                    }
                }
                previous_variable_size_data_type_size = variable_size_data_type_size;
			}
			data_type_number += 1;
        }
    }

    Ok(cat_def)
}

pub fn clean_pgtag_flag(reader: &Reader, db_page: &jet::DbPage, data: u16) -> u16 {
    // The upper 3-bits of the first 16-bit-value in the leaf page entry contain the page tag flags
    if reader.format_revision >= ESEDB_FORMAT_REVISION_EXTENDED_PAGE_HEADER
        && reader.page_size >= 16384
        && db_page.flags().contains(jet::PageFlags::IS_LEAF)
    {
        return data & 0x1FFF;
    }
    data
}

pub fn find_first_leaf_page(reader: &Reader, page_number: u32)
    -> Result<u32, SimpleError> {
    let db_page = jet::DbPage::new(reader, page_number)?;
    if db_page.flags().contains(jet::PageFlags::IS_LEAF) {
        return Ok(page_number);
    }

    let pg_tags = &db_page.page_tags;
    let child_page_number = page_tag_get_branch_child_page_number(reader, &db_page, &pg_tags[1])?;
    return find_first_leaf_page(reader, child_page_number);
}

pub fn load_data(
    reader: &Reader,
    tbl_def: &jet::TableDefinition,
    lv_tags: &Vec<LV_tags>,
    db_page: &jet::DbPage,
    page_tag_index: usize,
    column_id: u32,
    multi_value_index: usize // 0 value mean itagSequence = 1
) -> Result<Option<Vec<u8>>, SimpleError> {
    let pg_tags = &db_page.page_tags;

    let is_root = db_page.flags().contains(jet::PageFlags::IS_ROOT);
    if is_root {
        let _root_page_header = load_root_page_header(reader, &db_page, &pg_tags[0])?;
        //println!("root_page {:?}", _root_page_header);
    }

    if !db_page.flags().contains(jet::PageFlags::IS_LEAF) {
        return Err(SimpleError::new(format!("expected leaf page, page_flags 0x{:?}",
            db_page.flags())));
    }

    if page_tag_index == 0 || page_tag_index >= pg_tags.len() {
        return Err(SimpleError::new(format!("wrong page tag index: {}", page_tag_index)));
    }

    let page_tag = &pg_tags[page_tag_index];
    let page_offset = (db_page.page_number + 1) as u64 * reader.page_size;
    let mut offset = page_offset + db_page.size() as u64 + page_tag.offset as u64;
    let offset_start = offset;

    let mut first_word_readed = false;
    if page_tag.flags().intersects(jet::PageTagFlags::FLAG_HAS_COMMON_KEY_SIZE) {
        //let common_page_key_size : u16 = clean_pgtag_flag(reader, &db_page, reader.read_struct::<u16>(offset)?);
        first_word_readed = true;
        offset += 2;
    }
    let mut local_page_key_size : u16 = reader.read_struct(offset)?;
    if !first_word_readed {
        local_page_key_size = clean_pgtag_flag(reader, &db_page, local_page_key_size);
        first_word_readed = true;
    }
    offset += 2;
    offset += local_page_key_size as u64;

    let record_data_size = page_tag.size as u64 - (offset - offset_start);

    let offset_ddh = offset;
    let ddh = reader.read_struct::<ese_db::DataDefinitionHeader>(offset_ddh)?;
    offset += mem::size_of::<ese_db::DataDefinitionHeader>() as u64;

    let mut tagged_data_types_format = jet::TaggedDataTypesFormats::Index;
    if reader.format_version == 0x620 && reader.format_revision <= 2 {
        tagged_data_types_format = jet::TaggedDataTypesFormats::Linear;
    }

    let mut tagged_data_type_offset_bitmask : u16 = 0x3fff;
    if reader.format_revision >= ESEDB_FORMAT_REVISION_EXTENDED_PAGE_HEADER && reader.page_size >= 16384 {
        tagged_data_type_offset_bitmask = 0x7fff;
    }

    // read fixed data bits mask, located at the end of fixed columns
    let fixed_data_bits_mask_size = (ddh.last_fixed_size_data_type as usize + 7) / 8;
    let mut fixed_data_bits_mask : Vec<u8>= Vec::new();
    if fixed_data_bits_mask_size > 0 {
        fixed_data_bits_mask = reader.read_bytes(
            offset_ddh + ddh.variable_size_data_types_offset as u64 - fixed_data_bits_mask_size as u64,
            fixed_data_bits_mask_size)?;
    }

    let mut tagged_data_type_identifier : u16 = 0;
    let mut tagged_data_types_offset : u16 = 0;
    let mut tagged_data_type_offset : u16 = 0;
    let mut tagged_data_type_offset_data_size : u16 = 0;
    let mut previous_tagged_data_type_offset : u16 = 0;
    let mut remaining_definition_data_size : u16 = 0;
    let mut previous_variable_size_data_type_size : u16 = 0;

    let mut number_of_variable_size_data_types : u16 = 0;
    if ddh.last_variable_size_data_type > 127 {
        number_of_variable_size_data_types = ddh.last_variable_size_data_type as u16 - 127;
    }

    let mut current_variable_size_data_type : u32 = 127;
    let mut variable_size_data_type_offset = ddh.variable_size_data_types_offset;
    let mut variable_size_data_type_value_offset : u16 =
        (ddh.variable_size_data_types_offset + (number_of_variable_size_data_types * 2)).try_into().unwrap();
    for j in 0..tbl_def.column_catalog_definition_array.len() {
        let col = &tbl_def.column_catalog_definition_array[j];
        if col.identifier <= 127 {
            if col.identifier <= ddh.last_fixed_size_data_type as u32 {
                // fixed size column
                if col.identifier == column_id {
                    if fixed_data_bits_mask_size > 0 && fixed_data_bits_mask[j/8] & (1 << (j % 8)) > 0 {
                        // empty value
                        return Ok(None);
                    }
                    let v = reader.read_bytes(offset, col.size as usize)?;
                    return Ok(Some(v));
                }
                offset += col.size as u64;
            } else if col.identifier == column_id {
                // no value in tag
                return Ok(None);
            }
        } else if current_variable_size_data_type < ddh.last_variable_size_data_type as u32 {
            // variable size
            while current_variable_size_data_type < col.identifier {
                let variable_size_data_type_size : u16 = reader.read_struct(offset_ddh + variable_size_data_type_offset as u64)?;
                variable_size_data_type_offset += 2;
                current_variable_size_data_type += 1;
                if current_variable_size_data_type == col.identifier {
                    if (variable_size_data_type_size & 0x8000) == 0 {

                        if col.identifier == column_id {
                            let v = reader.read_bytes(offset_ddh + variable_size_data_type_value_offset as u64,
                                (variable_size_data_type_size - previous_variable_size_data_type_size) as usize)?;
                            return Ok(Some(v));
                        }

                        variable_size_data_type_value_offset += variable_size_data_type_size - previous_variable_size_data_type_size;
                        previous_variable_size_data_type_size = variable_size_data_type_size;
                    }
                }
                if current_variable_size_data_type >= ddh.last_variable_size_data_type as u32 {
                    break;
                }
            }
        } else {
            // tagged
            if tagged_data_types_format == jet::TaggedDataTypesFormats::Linear {
                // TODO
                println!("TODO tagged_data_types_format == jet::TaggedDataTypesFormats::Linear");
            } else if tagged_data_types_format == jet::TaggedDataTypesFormats::Index {
                if tagged_data_types_offset == 0 {
                    tagged_data_types_offset = variable_size_data_type_value_offset;
                    remaining_definition_data_size =
                        ((record_data_size - tagged_data_types_offset as u64) as u16).try_into().unwrap();

                    offset = offset_ddh + tagged_data_types_offset as u64;

                    if remaining_definition_data_size > 0 {
                        tagged_data_type_identifier = reader.read_struct::<u16>(offset)?;
                        offset += 2;

                        tagged_data_type_offset = reader.read_struct::<u16>(offset)?;
                        offset += 2;

                        if tagged_data_type_offset == 0 {
                            return Err(SimpleError::new("tagged_data_type_offset == 0"));
                        }
                        tagged_data_type_offset_data_size = (tagged_data_type_offset & 0x3fff) - 4;
                        remaining_definition_data_size -= 4;
                    }
                }
                if remaining_definition_data_size > 0 && col.identifier == tagged_data_type_identifier as u32 {
                    previous_tagged_data_type_offset = tagged_data_type_offset;
                    if tagged_data_type_offset_data_size > 0 {
                        tagged_data_type_identifier = reader.read_struct::<u16>(offset)?;
                        offset += 2;

                        tagged_data_type_offset = reader.read_struct::<u16>(offset)?;
                        offset += 2;

                        tagged_data_type_offset_data_size -= 4;
                        remaining_definition_data_size    -= 4;
                    }

                    let masked_previous_tagged_data_type_offset : u16 =
                        previous_tagged_data_type_offset & tagged_data_type_offset_bitmask;
                    let masked_tagged_data_type_offset = tagged_data_type_offset & tagged_data_type_offset_bitmask;

                    let mut tagged_data_type_size = 0;
                    if masked_tagged_data_type_offset > masked_previous_tagged_data_type_offset {
                        tagged_data_type_size = masked_tagged_data_type_offset - masked_previous_tagged_data_type_offset;
                    } else {
                        tagged_data_type_size = remaining_definition_data_size;
                    }
                    let mut tagged_data_type_value_offset = tagged_data_types_offset + masked_previous_tagged_data_type_offset;
                    let mut data_type_flags : u8 = 0;
                    if tagged_data_type_size > 0 {
                        remaining_definition_data_size -= tagged_data_type_size;
                        if (reader.format_revision >= ESEDB_FORMAT_REVISION_EXTENDED_PAGE_HEADER &&
                            reader.page_size >= 16384) || (previous_tagged_data_type_offset & 0x4000 ) != 0
                        {
                            data_type_flags = reader.read_struct(offset_ddh + tagged_data_type_value_offset as u64)?;

                            tagged_data_type_value_offset += 1;
                            tagged_data_type_size         -= 1;
                        }
                    }
                    if tagged_data_type_size > 0 && col.identifier == column_id {
                        use jet::TaggedDataTypeFlag;
                        offset = offset_ddh + tagged_data_type_value_offset as u64;
                        let dtf = TaggedDataTypeFlag::from_bits_truncate(data_type_flags as u16);
                        if dtf.intersects(TaggedDataTypeFlag::LONG_VALUE) {
                            let key = reader.read_struct::<u32>(offset)?;
                            let v = load_lv_data(reader, &lv_tags, key)?;
                            return Ok(Some(v));
                        } else if dtf.intersects(TaggedDataTypeFlag::MULTI_VALUE | TaggedDataTypeFlag::MULTI_VALUE_OFFSET) {
                            let mut mv_indexes : Vec<(u16/*shift*/, (bool/*lv*/, u16/*size*/))> = Vec::new();
                            if dtf.intersects(jet::TaggedDataTypeFlag::MULTI_VALUE_OFFSET) {
                                // The first byte contain the offset
                                // [13, ...]
                                let mut offset_mv_list = offset;
                                let value = reader.read_struct::<u8>(offset_mv_list)? as u16;
                                offset_mv_list += 1;

                                mv_indexes.push((1, (false, value)));
                                mv_indexes.push((value+1, (false, tagged_data_type_size - value - 1)));
                            } else if dtf.intersects(jet::TaggedDataTypeFlag::MULTI_VALUE) {
                                // The first 2 bytes contain the offset to the first value
                                // there is an offset for every value
                                // therefore first offset / 2 = the number of value entries
                                // [8, 0, 7, 130, 11, 2, 10, 131, ...]
                                let mut offset_mv_list = offset;
                                let mut value = reader.read_struct::<u16>(offset_mv_list)?;
                                offset_mv_list += 2;

                                let mut value_entry_size : u16 = 0;
                                let mut value_entry_offset = value & 0x7fff;
                                let mut entry_lvbit : bool = (value & 0x8000) > 0;
                                let number_of_value_entries = value_entry_offset / 2;

                                for _ in 1..number_of_value_entries {
                                    value = reader.read_struct::<u16>(offset_mv_list)?;
                                    offset_mv_list += 2;
                                    value_entry_size = (value & 0x7fff) - value_entry_offset;
                                    mv_indexes.push((value_entry_offset, (entry_lvbit, value_entry_size)));
                                    entry_lvbit = (value & 0x8000) > 0;
                                    value_entry_offset = value & 0x7fff;
                                }
                                value_entry_size = tagged_data_type_size - value_entry_offset;
                                mv_indexes.push((value_entry_offset, (entry_lvbit, value_entry_size)));
                            }
                            let mut mv_index = 0;
                            if multi_value_index > 0 && multi_value_index - 1 < mv_indexes.len() {
                                mv_index = multi_value_index - 1;
                            }

                            if mv_index < mv_indexes.len() {
                                let (shift, (lv, size)) = mv_indexes[mv_index];
                                let v;
                                if lv {
                                    let key = reader.read_struct::<u32>(offset + shift as u64)?;
                                    v = load_lv_data(reader, &lv_tags, key)?;
                                } else {
                                    v = reader.read_bytes(offset + shift as u64, size as usize)?;
                                }
                                return Ok(Some(v));
                            }
                        } else if dtf.intersects(jet::TaggedDataTypeFlag::COMPRESSED) {
                            const JET_wrnBufferTruncated: u32 = 1006;
                            const JET_errSuccess: u32 = 0;
                            let mut decompressed: u32 = 0;
                            let v = reader.read_bytes(offset, tagged_data_type_size as usize)?;
                            let mut res = unsafe { decompress(v.as_ptr(), v.len() as u32, ptr::null_mut(), 0, &mut decompressed) };

                            assert_eq!(res, JET_wrnBufferTruncated);

                            let mut  buf = Vec::<u8>::with_capacity(decompressed as usize);
                            unsafe { buf.set_len(buf.capacity()); }
                            res = unsafe { decompress(v.as_ptr(), v.len() as u32, buf.as_mut_ptr(), buf.len() as u32, &mut decompressed) };
                            assert_eq!(res, JET_errSuccess);

                            return Ok(Some(buf));
                        } else {
                            let v = reader.read_bytes(offset, tagged_data_type_size as usize)?;
                            return Ok(Some(v));
                        }
                    }
                }
            }
        }
        // column not found?
        if col.identifier == column_id {
            // default present?
            if col.default_value.len() > 0 {
                return Ok(Some(col.default_value.clone()));
            }
            // empty
            return Ok(None)
        }
    }

    Err(SimpleError::new(format!("column {} not found", column_id)))
}

pub fn load_lv_tag(
    reader: &Reader,
    db_page: &jet::DbPage,
    page_tag: &PageTag,
    page_tag_0: &PageTag
) -> Result<Option<LV_tags>, SimpleError> {
    let page_offset = (db_page.page_number + 1) as u64 * reader.page_size;
    let mut offset = page_offset + db_page.size() as u64 + page_tag.offset as u64;
    let page_tag_offset : u64 = offset;

    let mut res = LV_tags { common_page_key: vec![], local_page_key: vec![], key: 0, offset: 0, size: 0, seg_offset: 0 };

    let mut first_word_readed = false;
    let mut common_page_key_size : u16 = 0;
    if page_tag.flags().intersects(jet::PageTagFlags::FLAG_HAS_COMMON_KEY_SIZE) {
        common_page_key_size = clean_pgtag_flag(reader, db_page, reader.read_struct::<u16>(offset)?);
        first_word_readed = true;
        offset += 2;

        if common_page_key_size > 0 {
            let offset0 = page_offset + db_page.size() as u64 + page_tag_0.offset as u64;
            let mut common_page_key = reader.read_bytes(offset0, common_page_key_size as usize)?;
            res.common_page_key.append(&mut common_page_key);
        }
    }

    let mut local_page_key_size : u16 = reader.read_struct(offset)?;
    if !first_word_readed {
        local_page_key_size = clean_pgtag_flag(reader, db_page, local_page_key_size);
        first_word_readed = true;
    }
    offset += 2;
    if local_page_key_size > 0 {
        let mut local_page_key = reader.read_bytes(offset, local_page_key_size as usize)?;
        res.local_page_key.append(&mut local_page_key);
        offset += local_page_key_size as u64;
    }

    return if (page_tag.size as u64) - (offset - page_tag_offset) == 8 {
        let skey: u32 = reader.read_struct(offset)?;
        offset += 4;

        res.key = skey;

        let _total_size: u32 = reader.read_struct(offset)?;
        offset += 4;

        // TODO: handle? page_tags with skey & total_size only
        Ok(None)
    } else {
        let mut page_key: Vec<u8> = vec![];
        if common_page_key_size + local_page_key_size == 8 {
            page_key.append(&mut res.common_page_key.clone());
            page_key.append(&mut res.local_page_key.clone());
        } else if local_page_key_size >= 4 {
            page_key = res.local_page_key.clone();
        } else if common_page_key_size >= 4 {
            page_key = res.common_page_key.clone();
        }

        let skey = unsafe {
            match page_key[0..4].try_into() {
                Ok(pk) => std::mem::transmute::<[u8; 4], u32>(pk),
                Err(e) => return Err(SimpleError::new(format!("can't convert page_key {:?} into slice [0..4], error: {}",
                                                              page_key, e)))
            }
        }.to_be();

        res.key = skey;

        if page_key.len() == 8 {
            let segment_offset = unsafe {
                std::mem::transmute::<[u8; 4], u32>(page_key[4..8].try_into().unwrap())
            }.to_be();
            res.seg_offset = segment_offset;
        }

        res.offset = offset;
        res.size = (page_tag.size as u64 - (offset - page_tag_offset)).try_into().unwrap();

        Ok(Some(res))
    }
}

// TODO: change to map[key] = vec![], sorted by seg_offset
#[derive(Debug, Clone)]
pub struct LV_tags {
    pub common_page_key: Vec<u8>,
    pub local_page_key: Vec<u8>,
    pub key: u32,
    pub offset: u64,
    pub size: u32,
    pub seg_offset: u32,
}

impl std::fmt::Display for LV_tags{
    fn fmt (&self, fmt: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        write!(fmt, "common_page_key {:?}, local_page_key {:?}, key {}, offset {}, seg_offset {}, size {}",
            self.common_page_key, self.local_page_key, self.key, self.offset, self.seg_offset, self.size)
    }
}

pub fn load_lv_metadata(
    reader: &Reader,
    page_number: u32
) -> Result<Vec<LV_tags>, SimpleError> {
    let db_page = jet::DbPage::new(reader, page_number)?;
    let pg_tags = &db_page.page_tags;

    if !db_page.flags().contains(jet::PageFlags::IS_LONG_VALUE) {
        return Err(SimpleError::new(format!("pageno {}: IS_LONG_VALUE flag should be present",
            db_page.page_number)));
    }

    let is_root = db_page.flags().contains(jet::PageFlags::IS_ROOT);
    if is_root {
        let _root_page_header = load_root_page_header(reader, &db_page, &pg_tags[0])?;
        //println!("root_page {:?}", _root_page_header);
    }

    let mut res : Vec<LV_tags> = vec![];

    if !db_page.flags().contains(jet::PageFlags::IS_LEAF) {
        let mut prev_page_number = page_number;
        let mut page_number = page_tag_get_branch_child_page_number(reader, &db_page, &pg_tags[1])?;
        while page_number != 0 {
            let db_page = jet::DbPage::new(reader, page_number)?;
            let pg_tags = &db_page.page_tags;

            if db_page.prev_page() != 0 && prev_page_number != db_page.prev_page() {
                return Err(SimpleError::new(format!("pageno {}: wrong previous_page number {}, expected {}",
                    db_page.page_number, db_page.prev_page(), prev_page_number)));
            }
            if !db_page.flags().contains(jet::PageFlags::IS_LEAF | jet::PageFlags::IS_LONG_VALUE) {

                // maybe it's "Parent of leaf" page
                let r = load_lv_metadata(reader, page_number);
                match r {
                    Ok(mut r) => {
                        res.append(&mut r);
                    },
                    Err(e) => {
                        return Err(e);
                    }
                }
            } else {
                for i in 1..pg_tags.len() {
                    if jet::PageTagFlags::from_bits_truncate(pg_tags[i].flags).intersects(jet::PageTagFlags::FLAG_IS_DEFUNCT) {
                        continue;
                    }

                    match load_lv_tag(reader, &db_page, &pg_tags[i], &pg_tags[0]) {
                        Ok(r) => {
                            if let Some(lv_tag) = r {
                                res.push(lv_tag);
                            }
                        },
                        Err(e) => return Err(e)
                    }
                }
            }
            prev_page_number = page_number;
            page_number = db_page.next_page();
        }
    } else {
        for i in 1..pg_tags.len() {
            match load_lv_tag(reader, &db_page, &pg_tags[i], &pg_tags[0]) {
                Ok(r) => {
                    if let Some(lv_tag) = r {
                        res.push(lv_tag);
                    }
                },
                Err(e) => return Err(e)
            }
        }
    }

    Ok(res)
}

pub fn load_lv_data(
    reader: &Reader,
    lv_tags: &Vec<LV_tags>,
    long_value_key: u32,
) -> Result<Vec<u8>, SimpleError> {
    let mut res : Vec<u8> = vec![];
    let mut i = 0;
    while i < lv_tags.len() {
        if long_value_key == lv_tags[i].key && res.len() == lv_tags[i].seg_offset as usize {
            let mut v = reader.read_bytes(lv_tags[i].offset, lv_tags[i].size as usize)?;
            res.append(&mut v);
            i = 0; // search next seg_offset (lv_tags could be not sorted)
            continue;
        }
        i += 1;
    }

    if res.len() > 0 {
        Ok(res)
    } else {
        Err(SimpleError::new(format!("LV key {} not found", long_value_key)))
    }
}

#[allow(dead_code)]
mod test {
    use super::*;
    use std::{str, ffi::CString, ptr::null_mut, convert::TryFrom, collections::HashSet};
    use crate::esent::*;
    use crate::ese_parser;
    use crate::ese_trait::EseDb;
    use encoding::{all::{ASCII, UTF_16LE, UTF_8}, Encoding, EncoderTrap, DecoderTrap};

    macro_rules! jetcall {
        ($call:expr) => {
            unsafe {
                match $call {
                    0 => Ok(()),
                    err => Err(err),
                }
            }
        }
    }

    macro_rules! jettry {
        ($func:ident($($args:expr),*)) => {
            match jetcall!($func($($args),*)) {
                Ok(x) => x,
                Err(e) => panic!("{} failed: {}", stringify!($func), e),
            }
        }
    }

    fn size_of<T> () -> raw::c_ulong{
        mem::size_of::<T>() as raw::c_ulong
    }

    #[derive(Debug)]
    pub struct EseAPI {
        instance: JET_INSTANCE,
        sesid: JET_SESID,
        dbid: JET_DBID,
    }

    enum JET_CP {
        None = 0,
        Unicode = 1200,
        ASCII = 1252
    }

    impl TryFrom<u32> for JET_CP {
        type Error = ();

        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                x if x == JET_CP::None as u32 => Ok(JET_CP::None),
                x if x == JET_CP::ASCII as u32 => Ok(JET_CP::ASCII),
                x if x == JET_CP::Unicode as u32 => Ok(JET_CP::Unicode),
                _ => Err(()),
            }
        }
    }

    impl EseAPI {
        fn new(pg_size: usize) -> EseAPI {
            EseAPI::set_system_parameter_l(JET_paramDatabasePageSize, pg_size as u64);
            EseAPI::set_system_parameter_l(JET_paramDisableCallbacks, (true as u64).into());
            EseAPI::set_system_parameter_sz(JET_paramRecovery, "Off");

            let mut instance : JET_INSTANCE = 0;
            jettry!(JetCreateInstanceA(&mut instance, ptr::null()));
            jettry!(JetInit(&mut instance));

            let mut sesid : JET_SESID = 0;
            jettry!(JetBeginSessionA(instance, &mut sesid, ptr::null(), ptr::null()));

            EseAPI { instance, sesid, dbid: 0 }
        }

        fn set_system_parameter_l(paramId : u32, lParam: u64) {
            jettry!(JetSetSystemParameterA(ptr::null_mut(), 0, paramId, lParam, ptr::null_mut()));
        }

        fn set_system_parameter_sz(paramId : u32, szParam: &str) {
            jettry!(JetSetSystemParameterA(ptr::null_mut(), 0, paramId, 0, CString::new(szParam).unwrap().as_ptr()));
        }

        fn create_column(name: &str, col_type: JET_COLTYP, cp: JET_CP, grbit: JET_GRBIT) -> JET_COLUMNCREATE_A {
            println!("create_column: {}", name);

            JET_COLUMNCREATE_A{
                cbStruct: size_of::<JET_COLUMNCREATE_A>(),
                szColumnName: CString::new(name).unwrap().into_raw(),
                coltyp: col_type,
                cbMax: 0,
                grbit,
                cp: cp as u32,
                pvDefault: ptr::null_mut(), cbDefault: 0, columnid: 0, err: 0 }
        }

        fn create_num_column(name: &str, grbit: JET_GRBIT) -> JET_COLUMNCREATE_A {
            EseAPI::create_column(name, JET_coltypLong, JET_CP::None, grbit)
        }

        fn create_text_column(name: &str, cp: JET_CP, grbit: JET_GRBIT) -> JET_COLUMNCREATE_A {
            EseAPI::create_column(name, JET_coltypLongText, cp, grbit)
        }

        fn create_binary_column(name: &str, grbit: JET_GRBIT) -> JET_COLUMNCREATE_A {
            EseAPI::create_column(name, JET_coltypLongBinary, JET_CP::None, grbit)
        }

        fn create_table(self: &mut EseAPI,
                        name: &str,
                        columns: &mut Vec<JET_COLUMNCREATE_A>) -> JET_TABLEID {

            let mut table_def =  JET_TABLECREATE_A{
                        cbStruct: size_of::<JET_TABLECREATE_A>(),
                        szTableName: CString::new(name).unwrap().into_raw(),
                        szTemplateTableName: ptr::null_mut(),
                        ulPages: 0,
                        ulDensity: 0,
                        rgcolumncreate: columns.as_mut_ptr(),
                        cColumns: columns.len() as raw::c_ulong,
                        rgindexcreate: null_mut(),
                        cIndexes: 0,
                        grbit: 0,
                        tableid: 0,
                        cCreated: 0
                    };

            println!("create_table: {}", name);
            jettry!(JetCreateTableColumnIndexA(self.sesid, self.dbid, &mut table_def ));
            table_def.tableid
        }

        fn begin_transaction(self: &EseAPI) {
            jettry!(JetBeginTransaction(self.sesid));
        }

        fn commit_transaction(self: &EseAPI) {
            jettry!(JetCommitTransaction(self.sesid, 0));
        }
    }

    impl Drop for EseAPI {
        fn drop(&mut self) {
            println!("Dropping EseAPI");
            
            if self.sesid != 0 {
                jettry!(JetEndSession(self.sesid, 0));
            }
            if self.instance != 0 {
                jettry!(JetTerm2(self.instance, JET_bitTermComplete));
            }
        }
    }

    fn prepare_db(filename: &str, table: &str, pg_size: usize, record_size: usize, records_cnt: usize) -> PathBuf {
        let mut dst_path = PathBuf::from("testdata").canonicalize().unwrap();
        dst_path.push(filename);

        if dst_path.exists() {
            let _ = fs::remove_file(&dst_path);
        }

        println!("creating {}", dst_path.display());
        let mut db_client = EseAPI::new(pg_size);

        let dbpath = CString::new(dst_path.to_str().unwrap()).unwrap();
        jettry!(JetCreateDatabaseA(db_client.sesid, dbpath.as_ptr(), ptr::null(), &mut db_client.dbid, 0));

        let mut columns = Vec::<JET_COLUMNCREATE_A>::with_capacity(5);
        //columns.push(EseAPI::create_num_column("PK",JET_bitColumnAutoincrement));
        columns.push(EseAPI::create_text_column("compressed_unicode", JET_CP::Unicode, JET_bitColumnCompressed));
        columns.push(EseAPI::create_text_column("compressed_ascii", JET_CP::ASCII, JET_bitColumnCompressed));
        columns.push(EseAPI::create_binary_column("compressed_binary", JET_bitColumnCompressed));
        columns.push(EseAPI::create_text_column("usual_text", JET_CP::None, JET_bitColumnTagged));

        let tableid = db_client.create_table(table, &mut columns);

        for i in 0..records_cnt {
            let s = format!("Record {number:>width$}", number=i, width=record_size);

            db_client.begin_transaction();

            jettry!(JetPrepareUpdate(db_client.sesid, tableid, JET_prepInsert));
            for col in &columns {
                let data = match col.cp.try_into() {
                    Ok(JET_CP::Unicode) => match UTF_16LE.encode(&s, EncoderTrap::Strict) {
                        Ok(data) => data,
                        Err(e) => panic!("{}", e),
                    },
                    Ok(JET_CP::ASCII) => match ASCII.encode(&s, EncoderTrap::Strict) {
                        Ok(data) => data,
                        Err(e) => panic!("{}", e),
                    },
                    Ok(JET_CP::None) => match UTF_8.encode(&s, EncoderTrap::Strict) {
                        Ok(data) => data,
                        Err(e) => panic!("{}", e),
                    },
                    Err(e) => panic!("{:?}", e),
                };

                let mut setColumn = JET_SETCOLUMN {
                    columnid: col.columnid,
                    pvData: data.as_ptr() as *const raw::c_void,
                    cbData: data.len() as raw::c_ulong,
                    grbit: col.grbit,
                    ibLongValue: 0, itagSequence: 0, err: 0 };

                //println!("'{}' {}", s, s.len());

                jettry!(JetSetColumns(db_client.sesid, tableid, &mut setColumn, 1));
            }

            jettry!(JetUpdate(db_client.sesid, tableid, ptr::null_mut(), 0, ptr::null_mut()));
            db_client.commit_transaction();
        }

        dst_path
    }

    #[test]
    pub fn caching_test() -> Result<(), SimpleError> {
        let cache_size: usize = 10;
        let table = "test_table";
        let path = prepare_db("caching_test.edb", table, 1024 * 8, 1024, 4000);
        let mut reader = Reader::new(&path, cache_size as usize)?;
        let page_size = reader.page_size;
        let num_of_pages = std::cmp::min(fs::metadata(&path).unwrap().len() / page_size, page_size) as usize;
        let full_cache_size = 6 * cache_size;
        let stride = num_of_pages / full_cache_size;
        let chunk_size = page_size as usize / num_of_pages;
        let mut chunks = Vec::<Vec<u8>>::with_capacity(stride as usize);

        println!("cache_size: {}, page_size: {}, num_of_pages: {}, stride: {}, chunk_size: {}",
            cache_size, page_size, num_of_pages, stride, chunk_size);

        for pass in 1..3 {
            for pg_no in 1_usize..12_usize {
                let offset: u64 = (pg_no * (page_size as usize + chunk_size)) as u64;

                println!("pass {}, pg_no {}, offset {:x} ", pass, pg_no, offset);

                if pass == 1 {
                    let mut chunk = Vec::<u8>::with_capacity(stride as usize);
                    assert!(!reader.cache.get_mut().contains_key(&pg_no as &usize));
                    reader.read(offset, &mut chunk)?;
                    chunks.push(chunk);
                } else {
                    let mut chunk = Vec::<u8>::with_capacity(stride as usize);
                    // pg_no == 1 was deleted, because cache_size is 10 pages
                    // and we read 11, so least recently used page (1) was deleted
                    assert_eq!(reader.cache.get_mut().contains_key(&pg_no), pg_no != 1);
                    reader.read(offset, &mut chunk)?;
                    assert_eq!(chunk, chunks[pg_no as usize - 1]);
                }
            }
        }
        Ok(())
    }

     #[test]
    pub fn decompress_test() -> Result<(), SimpleError> {
        let table = "test_table";
        let path = prepare_db("decompress_test.edb", table, 1024 * 8, 10, 10);
        let mut jdb : ese_parser::EseParser = ese_parser::EseParser::init(5);

        match jdb.load(&path.to_str().unwrap()) {
             Some(e) => panic!("Error: {}", e),
             None => println!("Loaded {}", path.display())
        }

        let table_id = jdb.open_table(&table)?;
        let columns = jdb.get_columns(&table)?;
        let mut values = HashSet::<String>::new();

        for col in columns {
            print!("{}: ", col.name);
            match jdb.get_column_str(table_id, col.id, 0) {
                Ok(result) =>
                    if let Some(mut value) = result {
                        if col.cp == JET_CP::Unicode as u16 {
                            unsafe {
                                let buffer = slice::from_raw_parts(value.as_bytes() as *const _ as *const u16, value.len() / 2);
                                value = String::from_utf16(&buffer).unwrap();
                            }
                        }
                        if let Ok(s) = UTF_8.decode(&value.as_bytes(), DecoderTrap::Strict) {
                            value = s;
                        }
                        println!("{}", value);
                        values.insert(value);
                    }
                    else {
                        println!("column '{}' has no value", col.name);
                        values.insert("".to_string());
                    },
                Err(e) => panic!("error: {}", e),
            }
        }

        println!("values: {:?}", values);
        assert_eq!(values.len(), 1);

        Ok(())
    }

}