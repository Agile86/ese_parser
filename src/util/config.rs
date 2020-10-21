//config.rs
#![allow(deprecated)]
use log::{ debug };
use clap::{ Arg, App };

pub struct Config {
    pub inp_file: String,
    pub report_file: String,
}

impl Config {
    pub fn new() -> Result<Config, &'static str> {
        let matches = App::new("ESE DB dump")
            .version("0.1.0")
            .arg(Arg::with_name("in")
                .short("i")
                .long("input")
                .takes_value(true)
                .required(true)
                .help("Path to ESE db file"))
            .arg(Arg::with_name("out")
                .short("o")
                .long("output")
                .takes_value(true)
                .help("Path to output report"))
            .get_matches();

        let inp_file = matches.value_of("in").unwrap().to_owned();
        debug!(" inp_file: {}", inp_file);

        let report_file = matches.value_of("out").to_owned();
        match report_file {
            Some(s) => s,
            _ => &""
        };

        Ok(Config { inp_file, report_file : "".to_string()/*report_file.unwrap().to_string()*/ })
    }


    pub fn new_from_env(env_key: &str) -> Result<Config, String> {
        let path = std::env::var(env_key);

        if let Ok(inp_file) = path {
            if !inp_file.is_empty() {
                return Ok(Config { inp_file, report_file: "".to_string() });
            }
        }

        Err(format!("'{}' environment variable is not defined", env_key))
    }
}
