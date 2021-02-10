//#![feature(maybe_uninit_ref)]
#![allow(non_camel_case_types, clippy::mut_from_ref, clippy::cast_ptr_alignment)]
#[macro_use]
extern crate log;
extern crate strum;
mod ese;
mod util;

use std::process;

use ese_parser::ese::jet;
use ese_parser::util::config::Config;
use ese_parser::util::reader::*;

fn main() {
    env_logger::init();

    let config = Config::new().unwrap_or_else(|err| {
        error!("Problem parsing arguments: {}", err);
        process::exit(1);
    });
    info!("file '{}'", config.inp_file.display());

    let db_file_header = match load_db_file_header(&config) {
        Ok(x) => x,
        Err(e) => {
            error!("Application error: {}", e);
            process::exit(1);
        }
    };

    let io_handle = jet::IoHandle::new(&db_file_header);
    let db_page = jet::DbPage::new(&config, &io_handle, 13 as u32);
    let pg_tags = load_page_tags(&config, &io_handle, &db_page).unwrap();
    println!("page {:?}, size: {}", db_page, db_page.size());

    if db_page.common().page_flags.contains(jet::PageFlags::IS_ROOT) {
        let root_page_header = load_root_page_header(&config, &io_handle, &db_page, &pg_tags[0]).unwrap();
        println!("root_page {:?}", root_page_header);
    }
    load_data_definition(&config, &io_handle, &db_page, &pg_tags[1]);
    for pg_tag in pg_tags {
        println!("tag {:?}", pg_tag);
    }
}
