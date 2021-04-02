use super::*;
use std::collections::HashSet;
use crate::ese_parser::EseParser;
use crate::ese_trait::*;
use encoding::{all::UTF_8, Encoding};

#[cfg(target_os = "windows")]
use crate::parser::reader::gen_db::*;

#[cfg(target_os = "linux")]
pub fn prepare_db(filename: &str, table: &str, pg_size: usize, record_size: usize, records_cnt: usize) -> PathBuf {
    let mut dst_path = PathBuf::from("testdata").canonicalize().unwrap();
    dst_path.push("decompress_test.edb");
    dst_path
}

#[cfg(target_os = "linux")]
pub fn clean_db(dst_path: &PathBuf) {
}

#[test]
pub fn caching_test() -> Result<(), SimpleError> {
    let cache_size: usize = 10;
    let table = "test_table";
    let path = prepare_db("caching_test.edb", table, 1024 * 8, 1024, 1000);
    let mut reader = Reader::new(&path, cache_size as usize)?;
    let page_size = reader.page_size as u64;
    let num_of_pages = std::cmp::min(fs::metadata(&path).unwrap().len() / page_size, page_size) as usize;
    let full_cache_size = 6 * cache_size;
    let stride = num_of_pages / full_cache_size;
    let chunk_size = page_size as usize / num_of_pages;
    let mut chunks = Vec::<Vec<u8>>::with_capacity(stride as usize);

    println!("cache_size: {}, page_size: {}, num_of_pages: {}, stride: {}, chunk_size: {}",
        cache_size, page_size, num_of_pages, stride, chunk_size);

    for pass in 1..3 {
        for pg_no in 1_u32..12_u32 {
            let offset: u64 = (pg_no as u64 * (page_size + chunk_size as u64));

            println!("pass {}, pg_no {}, offset {:x} ", pass, pg_no, offset);

            if pass == 1 {
                let mut chunk = Vec::<u8>::with_capacity(stride as usize);
                assert!(!reader.cache.get_mut().contains_key(&pg_no));
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
    clean_db(&path);
    Ok(())
}

fn check_row(jdb: &mut EseParser, table_id: u64, columns: &Vec<ColumnInfo>) -> HashSet<String> {
    let mut values = HashSet::<String>::new();

    for col in columns {
        match jdb.get_column_str(table_id, col.id, 0) {
            Ok(result) =>
                if let Some(mut value) = result {
                    if col.cp == 1200 {
                        unsafe {
                            let buffer = slice::from_raw_parts(value.as_bytes() as *const _ as *const u16, value.len() / 2);
                            value = String::from_utf16(&buffer).unwrap();
                        }
                    }
                    if let Ok(s) = UTF_8.decode(&value.as_bytes(), encoding::DecoderTrap::Strict) {
                        value = s;
                    }
                    values.insert(value);
                } else {
                    println!("column '{}' has no value", col.name);
                    values.insert("".to_string());
                },
            Err(e) => panic!("error: {}", e),
        }
    }
    values
}

#[cfg(target_os = "windows")]
#[test]
pub fn decompress_test() -> Result<(), SimpleError> {
    let table = "test_table";
    let path = prepare_db("decompress_test.edb", table, 1024 * 8, 10, 10);
    let mut jdb : EseParser = EseParser::init(5);

    match jdb.load(&path.to_str().unwrap()) {
        Some(e) => panic!("Error: {}", e),
        None => println!("Loaded {}", path.display())
    }

    let table_id = jdb.open_table(&table)?;
    let columns = jdb.get_columns(&table)?;

    assert!(jdb.move_row(table_id, ESE_MoveFirst));

    for rec_no in 0.. {
        let values = check_row(&mut jdb, table_id, &columns);

        println!("{}: {:?}", rec_no, values);
        assert_eq!(values.len(), 1);

        if !jdb.move_row(table_id, ESE_MoveNext) {
            break;
        }
    }
    clean_db(&path);
    Ok(())
}

