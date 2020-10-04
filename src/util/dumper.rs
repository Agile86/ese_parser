﻿//dumper.rs

pub use prettytable::{Table, Row, Cell};
extern crate hexdump;
use itertools::Itertools;
use std::fmt;
use std::string::ToString;

use crate::ese::db_file_header::{ esedb_file_header };
use crate::ese::esent::{JET_DBINFOMISC, JET_SIGNATURE, JET_LOGTIME};

impl fmt::Debug for JET_LOGTIME {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        unsafe {
            fmt.debug_struct("JET_LOGTIME")
                .field("bSeconds", &self.bSeconds)
                .field("bMinutes", &self.bMinutes)
                .field("bHours", &self.bHours)
                .field("bDay", &self.bDay)
                .field("bMonth", &self.bMonth)
                .field("bYear", &self.bYear)
                .field("fTimeIsUTC", &self.__bindgen_anon_1.__bindgen_anon_1.fTimeIsUTC())
                .finish()
        }
    }
}

impl fmt::Debug for JET_SIGNATURE {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        unsafe {
            fmt.debug_struct("JET_SIGNATURE")
                .field("ulRandom", &self.ulRandom)
                .field("logtimeCreate", &self.logtimeCreate)
                .field("szComputerName", &self.szComputerName)
                .finish()
        }
    }
}

impl fmt::Debug for JET_DBINFOMISC {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("JET_DBINFOMISC")
            .field("ulVersion", &self.ulVersion)
            .field("ulUpdate", &self.ulUpdate)
            .field("signDb", &self.signDb)
            .finish()
    }
}


pub fn dump_db_file_header(db_file_header: esedb_file_header) {
    let mut table = Table::new();

    macro_rules! add_row {
        ($fld: expr, $val: expr) => {table.add_row(Row::new(vec![Cell::new($fld), Cell::new($val)]))}
    }

    macro_rules! add_field {
        ($fld: ident) => {
            let s = format!("{:#x}", db_file_header.$fld);
            add_row!(stringify!($fld), &s)
        };
    }

    macro_rules! add_enum_field {
        ($fld: ident) => {
            let s = format!("{} ({})", db_file_header.$fld, db_file_header.$fld as u8);
            add_row!(stringify!($fld), &s)
        };
    }

    macro_rules! add_dt_field {
        ($dt: ident) => {
            let y = if db_file_header.$dt[5] > 0 {1900 + db_file_header.$dt[5] as u16} else {0};
            let s = format!("{:0>2}/{:0>2}/{:0>4} {:0>2}:{:0>2}:{:0>2}",
                            db_file_header.$dt[4], db_file_header.$dt[3], y,
                            db_file_header.$dt[2], db_file_header.$dt[1], db_file_header.$dt[0],);
            add_row!(stringify!($dt), &s);
        }
    }
    macro_rules! add_64_field {
        ($fld: ident) => {
            let s = u64::from_be_bytes(db_file_header.$fld).to_string();
            add_row!(stringify!($fld), &s);
        }
    }
    macro_rules! add_hex_field {
        ($fld: ident) => {
            let mut s: String = "".to_string();
            hexdump::hexdump_iter(&db_file_header.$fld).foreach(|line| { s.push_str(&line); s.push_str("\n"); } );
            add_row!(stringify!($fld), &s);
        }
    }
    macro_rules! add_sign_field {
        ($fld: ident) => {
            let sign = &db_file_header.$fld;
            let dt = &sign.logtime_create;
            let s = format!("Create time:{}/{}/{} {}:{}:{} Rand:{} Computer: {}",
                                dt.month, dt.day, dt.year as u32 + 1900, dt.hours, dt.minutes, dt.seconds,
                                sign.random, std::str::from_utf8(&sign.computer_name).unwrap());
            add_row!(stringify!($fld), &s);
        }
    }

    add_field!(checksum);
    add_field!(signature);
    add_field!(format_version);
    add_field!(file_type);
    add_hex_field!(database_time);
    add_sign_field!(database_signature);
    add_enum_field!(database_state);
    add_64_field!(consistent_postition);
    add_dt_field!(consistent_time);
    add_dt_field!(attach_time);
    add_64_field!(attach_postition);
    add_dt_field!(detach_time);
    add_64_field!(detach_postition);
    add_field!(unknown1);
    add_sign_field!(log_signature);
    add_hex_field!(previous_full_backup);
    add_hex_field!(previous_incremental_backup);
    add_hex_field!(current_full_backup);
    add_field!(shadowing_disabled);
    add_field!(last_object_identifier);
    add_field!(index_update_major_version);
    add_field!(index_update_minor_version);
    add_field!(index_update_build_number);
    add_field!(index_update_service_pack_number);
    add_field!(format_revision);
    add_field!(page_size);
    add_dt_field!(repair_time);
    add_sign_field!(unknown2);
    add_dt_field!(scrub_database_time);
    add_dt_field!(scrub_time);
    add_hex_field!(required_log);
    add_field!(upgrade_exchange5_format);
    add_field!(upgrade_free_pages);
    add_field!(upgrade_space_map_pages);
    add_hex_field!(current_shadow_volume_backup);
    add_field!(creation_format_version);
    add_field!(creation_format_revision);
    add_hex_field!(unknown3);
    add_field!(old_repair_count);
    add_field!(ecc_fix_success_count);
    add_dt_field!(ecc_fix_success_time);
    add_field!(old_ecc_fix_success_count);
    add_field!(ecc_fix_error_count);
    add_dt_field!(ecc_fix_error_time);
    add_field!(old_ecc_fix_error_count);
    add_field!(bad_checksum_error_count);
    add_dt_field!(bad_checksum_error_time);
    add_field!(old_bad_checksum_error_count);
    add_field!(committed_log);
    add_hex_field!(previous_shadow_volume_backup);
    add_hex_field!(previous_differential_backup);
    add_hex_field!(unknown4_1);
    add_hex_field!(unknown4_2);
    add_field!(nls_major_version);
    add_field!(nls_minor_version);
    add_hex_field!(unknown5_1);
    add_hex_field!(unknown5_2);
    add_hex_field!(unknown5_3);
    add_hex_field!(unknown5_4);
    add_hex_field!(unknown5_5);
    add_field!(unknown_flags);

    table.printstd();
}
